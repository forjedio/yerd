//! Per-session GUI diagnostic log + the tracing subscriber that feeds it.
//!
//! The GUI host process has no durable log of its own otherwise: under a bundled
//! `.app` / login launch its stderr is discarded, so when the daemon won't come
//! up there's nothing to inspect. This module installs a tracing subscriber that
//! writes a single **truncate-on-launch** file at `{cache}/yerd-gui.log`
//! (user-owned on macOS + Linux — no permission traps), capturing every
//! command/action/warning/error in the daemon install/upgrade/start flow at
//! `DEBUG`, interleaved with frontend lines pushed via [`gui_log`].
//!
//! Two reset triggers keep it from growing unbounded:
//! - **Per session:** the file is truncated when the process starts (a fresh
//!   `File::create` in [`SessionLog::create`]).
//! - **Staleness:** if the file ages past [`MAX_AGE`] it is re-truncated on the
//!   next write (a long-running GUI never accumulates more than an hour of logs).
//!
//! The `About → GUI Logs` dialog reads this file plus a tail of the daemon's own
//! `yerdd.<date>.log` via [`get_gui_logs`].

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry;

use crate::error::GuiError;

/// Re-truncate the session log once it ages past this, so a long-running GUI
/// session can never accumulate more than ~an hour of `DEBUG` output.
const MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Tail caps for the dialog. The session file is bounded by the 1 h reset but a
/// busy hour at `DEBUG` can still be MBs, so the GUI tail is byte-bounded too.
const GUI_TAIL_BYTES: u64 = 256 * 1024;
const GUI_TAIL_LINES: usize = 2000;
const DAEMON_TAIL_LINES: usize = 200;

/// Mutable interior of a [`SessionLog`]: the current file handle and the instant
/// it was (re)created, behind the lock so concurrent tracing writers serialise.
struct State {
    /// `None` if the file could not be (re)created — writes then degrade to a
    /// no-op rather than erroring (logging must never break a UI flow).
    file: Option<File>,
    epoch: Instant,
}

/// A truncate-on-create, reset-when-stale session log file. Installed directly
/// into the global `registry()` as a [`MakeWriter`]; the registry is `'static`
/// and owns it for the whole process, so there is no flush guard to keep alive.
pub struct SessionLog {
    path: PathBuf,
    max_age: Duration,
    inner: Mutex<State>,
}

impl SessionLog {
    /// Create the log at `path`, truncating any existing file (the per-session
    /// reset). Best-effort: a create failure leaves `file: None` and the writer
    /// degrades to a no-op.
    fn create(path: PathBuf, max_age: Duration) -> Self {
        let file = File::create(&path).ok();
        Self {
            path,
            max_age,
            inner: Mutex::new(State {
                file,
                epoch: Instant::now(),
            }),
        }
    }
}

/// The per-event writer the fmt layer formats into. Holds the lock for the whole
/// (single) log line, so the staleness check + write can't interleave with
/// another thread's event.
pub struct SessionWriter<'a>(std::sync::MutexGuard<'a, State>);

impl io::Write for SessionWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.file.as_mut() {
            Some(f) => f.write(buf),
            None => Ok(buf.len()), // degrade silently; never error/panic
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.0.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for SessionLog {
    type Writer = SessionWriter<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        // Poison-tolerant: a panicked writer thread must not take logging down.
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        // Staleness reset — checked once per event (here), before the line's
        // bytes, so it can never truncate between the partial writes of one line.
        if state.epoch.elapsed() >= self.max_age {
            state.file = File::create(&self.path).ok();
            state.epoch = Instant::now();
        }
        SessionWriter(state)
    }
}

/// Resolve `{cache}/yerd-gui.log`, creating the cache dir. Same dir as the
/// daemon's `yerdd.<date>.log`, so the dialog reads both from one place.
fn resolve_log_path() -> Result<PathBuf, GuiError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| GuiError::internal(format!("cannot resolve cache dir: {e}")))?;
    std::fs::create_dir_all(&dirs.cache)
        .map_err(|e| GuiError::internal(format!("cannot create {}: {e}", dirs.cache.display())))?;
    Ok(dirs.cache.join("yerd-gui.log"))
}

/// Install the GUI tracing subscriber: a compact stderr layer (so `cargo run`
/// still shows logs live) plus the truncating session-file layer. Always
/// `DEBUG`, independent of build profile, so release builds capture diagnostics
/// too. Call once, as the first thing in `main()`. Degrades to stderr-only if
/// the cache dir can't be resolved/created; never panics.
pub fn init() {
    let filter = Targets::new().with_default(LevelFilter::DEBUG);

    let stderr_layer = fmt::layer()
        .with_writer(io::stderr)
        .compact()
        .with_filter(filter.clone());

    let file_layer = match resolve_log_path() {
        Ok(path) => {
            let session = SessionLog::create(path, MAX_AGE);
            Some(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(session)
                    .with_filter(filter),
            )
        }
        Err(e) => {
            eprintln!("yerd-gui: could not open session log: {e}; logging to stderr only");
            None
        }
    };

    // Swallow the `try_init` error: it only fires if a global subscriber already
    // exists (nothing else in this crate installs one), the desired no-op.
    let _ = registry().with(stderr_layer).with(file_layer).try_init();
}

/// Append a line to the session log from the frontend. Synchronous, infallible,
/// no daemon IPC — it just emits into the global subscriber so the line lands in
/// the same file, interleaved with the host's own events.
#[tauri::command]
pub fn gui_log(level: String, message: String) {
    match level.as_str() {
        "error" => tracing::error!(target: "yerd_gui::frontend", "{message}"),
        "warn" => tracing::warn!(target: "yerd_gui::frontend", "{message}"),
        "debug" => tracing::debug!(target: "yerd_gui::frontend", "{message}"),
        _ => tracing::info!(target: "yerd_gui::frontend", "{message}"),
    }
}

/// The GUI session log + a tail of the daemon's own rolling log, for the
/// `About → GUI Logs` dialog. Paths are surfaced so the dialog can show "where".
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiLogs {
    gui_path: Option<String>,
    gui_log: Vec<String>,
    daemon_path: Option<String>,
    daemon_log: Vec<String>,
}

/// Read both logs (off the runtime — filesystem work).
#[tauri::command]
pub async fn get_gui_logs() -> Result<GuiLogs, GuiError> {
    tokio::task::spawn_blocking(read_gui_logs)
        .await
        .map_err(|e| GuiError::internal(format!("reading logs failed: {e}")))
}

fn read_gui_logs() -> GuiLogs {
    use yerd_platform::{ActivePaths, Paths};
    let cache = ActivePaths::new().resolve().ok().map(|d| d.cache);

    let gui_path = cache.as_ref().map(|c| c.join("yerd-gui.log"));
    let gui_log = gui_path
        .as_ref()
        .map(|p| tail_file_bounded(p, GUI_TAIL_BYTES, GUI_TAIL_LINES))
        .unwrap_or_default();

    // Reuse the daemon module's single rolling-log finder + line tail.
    let daemon_path = cache
        .as_ref()
        .and_then(|c| crate::daemon::newest_rolling_log(c));
    let daemon_log = daemon_path
        .as_ref()
        .map(|p| crate::daemon::tail_lines(p, DAEMON_TAIL_LINES))
        .unwrap_or_default();

    GuiLogs {
        gui_path: gui_path.map(|p| p.display().to_string()),
        gui_log,
        daemon_path: daemon_path.map(|p| p.display().to_string()),
        daemon_log,
    }
}

/// Tail the last `max_lines` of `path`, reading at most the final `max_bytes`.
/// Best-effort: a missing/unreadable file → empty. Decodes with
/// `from_utf8_lossy` and drops the first line **only when the read began
/// mid-line** (the byte before the window isn't a newline), so a mid-codepoint
/// seek start is safe while a boundary-aligned seek keeps its whole first line.
/// Takes no lock vs. the writer — a benign display race (worst case: a clipped
/// final line).
fn tail_file_bounded(path: &Path, max_bytes: u64, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    // Seek one byte *before* the window (when not already at the file start) so
    // the byte preceding it reveals whether the read began on a line boundary.
    let seek_to = if start > 0 {
        start.saturating_sub(1)
    } else {
        0
    };
    if start > 0 && f.seek(SeekFrom::Start(seek_to)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    // When we seeked, `buf`'s first byte is the one before the window: a '\n'
    // means the window starts exactly on a line boundary (first line is whole —
    // keep it), anything else means the read began mid-line (first line is a
    // partial tail — drop it). Peel that probe byte off either way.
    let (drop_first, body) = match buf.split_first() {
        Some((&first, rest)) if start > 0 => (first != b'\n', rest),
        _ => (false, buf.as_slice()),
    };
    let text = String::from_utf8_lossy(body);
    let mut lines: Vec<&str> = text.lines().collect();
    if drop_first && !lines.is_empty() {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(max_lines);
    lines.iter().skip(from).map(|s| (*s).to_owned()).collect()
}

// ── Diagnostics payload (About → Diagnostics) ───────────────────────────────

/// Cap on how many ERROR lines from each log to include in the payload.
const DIAG_ERROR_LINES: usize = 100;
/// Tail window scanned for ERROR lines (the daemon log can be a full day).
const DIAG_SCAN_BYTES: u64 = 1024 * 1024;
/// Trailing lines of the macOS self-repair log to include (it's capped at 64 KiB).
const REPAIR_TAIL_LINES: usize = 200;
/// Payload shape version, so a pasted blob is identifiable later.
const DIAG_SCHEMA: u32 = 1;
/// Bound for each diagnostics IPC probe — the daemon may be down/wedged (the
/// common diagnostic case), and the probe must never hang the command.
const DIAG_IPC_TIMEOUT: Duration = Duration::from_secs(4);

/// Resolved on-disk locations, for the diagnostics payload.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagPaths {
    config: Option<String>,
    data: Option<String>,
    state: Option<String>,
    cache: Option<String>,
    runtime: Option<String>,
    socket: Option<String>,
    socket_exists: bool,
    yerdd: Option<String>,
    gui_log: Option<String>,
    daemon_log: Option<String>,
    spawn_log: Option<String>,
}

/// A self-contained snapshot of everything useful for diagnosing why Yerd isn't
/// behaving: versions, resolved paths, the service-manager configuration
/// (`launchctl print` / `systemctl status`), the daemon's runtime status + doctor
/// findings (when reachable), and the ERROR lines from each log.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    /// Payload shape version, so a pasted blob is identifiable.
    schema: u32,
    /// Unix seconds when this snapshot was taken.
    generated_at_unix: u64,
    app_version: String,
    host_os: String,
    arch: String,
    /// Whether the daemon answered an IPC probe. Diagnostics is most useful when
    /// it's down, so this degrades gracefully and keeps every host-side field.
    daemon_reachable: bool,
    service_manager: String,
    /// `launchctl print gui/<uid>/dev.yerd.daemon` (macOS) or `systemctl --user
    /// status yerd` (Linux) — the daemon's service-manager configuration/status.
    service_status: Option<String>,
    pending_approval: bool,
    translocated: bool,
    daemon_registered_version: Option<String>,
    daemon_version_conflict: Option<String>,
    paths: DiagPaths,
    /// The daemon's runtime status (ports, DNS, CA, PHP, services …) when
    /// reachable. Carries no secrets — the CA section is path + fingerprint +
    /// trust bool only, never key material. `None` when the daemon is down.
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<Box<yerd_ipc::StatusReport>>,
    /// Doctor findings (code / severity / title / detail / remedy). Empty when the
    /// daemon is unreachable.
    doctor: Vec<yerd_ipc::Diagnosis>,
    gui_log_errors: Vec<String>,
    daemon_log_errors: Vec<String>,
    spawn_log_errors: Vec<String>,
    /// Full tail of the macOS SMAppService self-repair trail — the GUI's only
    /// record of re-registration attempts/outcomes (empty off macOS / when absent).
    repair_log: Vec<String>,
}

/// Build the diagnostics JSON. Probes the daemon (bounded — it may be down),
/// then does the host-side filesystem/subprocess gathering off the runtime.
#[tauri::command]
pub async fn get_diagnostics() -> Result<String, GuiError> {
    let (status, doctor) = tokio::join!(probe_status(), probe_doctor());
    tokio::task::spawn_blocking(move || build_diagnostics_json(status, doctor))
        .await
        .map_err(|e| GuiError::internal(format!("gathering diagnostics failed: {e}")))?
}

/// The daemon's runtime status via a bounded IPC probe, or `None` if unreachable.
async fn probe_status() -> Option<Box<yerd_ipc::StatusReport>> {
    match crate::ipc::exchange_timeout(&yerd_ipc::Request::Status, DIAG_IPC_TIMEOUT).await {
        Ok(yerd_ipc::Response::Status { report }) => Some(report),
        _ => None,
    }
}

/// The doctor findings via a bounded IPC probe, or empty if unreachable.
async fn probe_doctor() -> Vec<yerd_ipc::Diagnosis> {
    match crate::ipc::exchange_timeout(&yerd_ipc::Request::Diagnose, DIAG_IPC_TIMEOUT).await {
        Ok(yerd_ipc::Response::Diagnoses { items }) => items,
        _ => Vec::new(),
    }
}

fn build_diagnostics_json(
    status: Option<Box<yerd_ipc::StatusReport>>,
    doctor: Vec<yerd_ipc::Diagnosis>,
) -> Result<String, GuiError> {
    serde_json::to_string_pretty(&gather_diagnostics(status, doctor))
        .map_err(|e| GuiError::internal(format!("serialize diagnostics: {e}")))
}

/// ERROR lines from the tail of `path` (best-effort; empty when absent).
fn error_lines(path: Option<&Path>, max: usize) -> Vec<String> {
    let Some(path) = path else {
        return Vec::new();
    };
    // Scan a generous tail, then keep only the lines a `tracing` ERROR event
    // prints (the level token appears verbatim in the compact/file format).
    let mut errs: Vec<String> = tail_file_bounded(path, DIAG_SCAN_BYTES, usize::MAX)
        .into_iter()
        .filter(|l| l.contains("ERROR"))
        .collect();
    if errs.len() > max {
        errs = errs.split_off(errs.len() - max);
    }
    errs
}

fn gather_diagnostics(
    status: Option<Box<yerd_ipc::StatusReport>>,
    doctor: Vec<yerd_ipc::Diagnosis>,
) -> Diagnostics {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new().resolve().ok();
    let show = |p: &std::path::Path| p.display().to_string();

    let socket = dirs.as_ref().map(|d| d.runtime.join("yerd.sock"));
    let cache = dirs.as_ref().map(|d| d.cache.clone());
    let gui_log = cache.as_ref().map(|c| c.join("yerd-gui.log"));
    let daemon_log = cache
        .as_ref()
        .and_then(|c| crate::daemon::newest_rolling_log(c));
    let spawn_log = cache.as_ref().map(|c| c.join("yerdd-spawn.log"));

    let paths = DiagPaths {
        config: dirs.as_ref().map(|d| show(&d.config)),
        data: dirs.as_ref().map(|d| show(&d.data)),
        state: dirs.as_ref().map(|d| show(&d.state)),
        cache: cache.as_ref().map(|c| show(c)),
        runtime: dirs.as_ref().map(|d| show(&d.runtime)),
        socket_exists: socket.as_ref().map(|s| s.exists()).unwrap_or(false),
        socket: socket.as_ref().map(|s| show(s)),
        yerdd: crate::daemon::resolve_yerdd().as_ref().map(|p| show(p)),
        gui_log: gui_log.as_ref().map(|p| show(p)),
        daemon_log: daemon_log.as_ref().map(|p| show(p)),
        spawn_log: spawn_log.as_ref().map(|p| show(p)),
    };

    let repair_log = cache
        .as_ref()
        .map(|c| crate::daemon::tail_lines(&c.join("yerd-gui-repair.log"), REPAIR_TAIL_LINES))
        .unwrap_or_default();

    let generated_at_unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Diagnostics {
        schema: DIAG_SCHEMA,
        generated_at_unix,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        host_os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        daemon_reachable: status.is_some(),
        service_manager: crate::autostart::service_manager_label().to_owned(),
        service_status: crate::autostart::service_status_text(),
        pending_approval: crate::autostart::daemon_pending_approval(),
        translocated: diag_translocated(),
        daemon_registered_version: crate::autostart::daemon_registered_version(),
        daemon_version_conflict: crate::autostart::daemon_version_conflict(),
        paths,
        status,
        doctor,
        gui_log_errors: error_lines(gui_log.as_deref(), DIAG_ERROR_LINES),
        daemon_log_errors: error_lines(daemon_log.as_deref(), DIAG_ERROR_LINES),
        spawn_log_errors: error_lines(spawn_log.as_deref(), DIAG_ERROR_LINES),
        repair_log,
    }
}

/// "Running from a non-installed location" (always false off macOS).
fn diag_translocated() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::autostart::is_translocated()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn truncates_existing_file_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.log");
        std::fs::write(&path, b"stale content from a previous session").unwrap();
        let _log = SessionLog::create(path.clone(), MAX_AGE);
        assert_eq!(
            std::fs::read(&path).unwrap().len(),
            0,
            "create must truncate"
        );
    }

    #[test]
    fn resets_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.log");
        // max_age = ZERO → every event is "stale" and re-truncates first.
        let log = SessionLog::create(path.clone(), Duration::ZERO);
        log.make_writer().write_all(b"a\n").unwrap();
        log.make_writer().write_all(b"b\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b\n");
    }

    #[test]
    fn keeps_lines_within_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.log");
        let log = SessionLog::create(path.clone(), MAX_AGE);
        log.make_writer().write_all(b"a\n").unwrap();
        log.make_writer().write_all(b"b\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
    }

    #[test]
    fn bounded_tail_drops_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.log");
        std::fs::write(&path, b"line1\nline2\nline3\n").unwrap();
        // Tiny byte budget forces a mid-file seek; the partial first line drops.
        let out = tail_file_bounded(&path, 8, 100);
        assert_eq!(out, vec!["line3".to_owned()]);
    }

    #[test]
    fn bounded_tail_keeps_boundary_aligned_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.log");
        std::fs::write(&path, b"line1\nline2\nline3\n").unwrap();
        // A 12-byte budget seeks to offset 6 — exactly after the first '\n', so the
        // window opens on a line boundary and the whole first line must be kept
        // (the old unconditional remove(0) wrongly dropped "line2" here).
        let out = tail_file_bounded(&path, 12, 100);
        assert_eq!(out, vec!["line2".to_owned(), "line3".to_owned()]);
    }
}
