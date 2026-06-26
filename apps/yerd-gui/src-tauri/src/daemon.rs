//! `yerdd` lifecycle from the GUI: locate, start, stop.
//!
//! All host-side — the daemon may be down when these run. Mirrors `elevate.rs`:
//! resolve trusted binaries relative to our own exe, do blocking work off the
//! async runtime, and thread every failure through [`GuiError`] (the crate bans
//! `unwrap`/`expect`/`panic` under clippy). The OS service mechanism
//! (systemd/launchd/SMAppService) lives in [`crate::autostart`]; this module owns
//! binary resolution, the start/stop orchestration, and the optional
//! "install the bundled CLI on PATH" helper. The daemon binary is **bundled**
//! inside the app (Tauri `externalBin`) — there is no runtime download.

use std::path::PathBuf;

use crate::error::GuiError;

// ── binary resolution ───────────────────────────────────────────────────────

/// `$HOME`, or `None` if unset.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Directories searched for a binary, in priority order, after the
/// beside-`current_exe` check.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/bin"));
    dirs
}

/// Resolve a bundled binary: first beside our own executable (macOS
/// `Contents/MacOS/`; Linux `.deb` symlinks `yerd`/`yerdd`/`yerd-helper` into
/// `/usr/bin` beside `yerd-gui`), then the usual dirs. Mirrors
/// `bin/yerd/src/elevate.rs::sibling_binaries`.
pub(crate) fn resolve_binary(name: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    if let Some(found) = search_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|c| c.is_file())
    {
        return Some(found);
    }
    // Linux: the `.deb` installs the Tauri sidecars under `/usr/lib/<product>/`
    // and the postinst *symlinks* them into `/usr/bin`. That symlink is not
    // dpkg-tracked and the postinst fails closed (a leftover `/usr/bin/yerd`
    // from the v1 Go project, or a Tauri path change, aborts it), so search the
    // real install dir directly as a fallback — otherwise a postinst hiccup
    // leaves `yerdd` present on disk but unfindable and the daemon "won't start".
    #[cfg(target_os = "linux")]
    {
        lib_sidecar(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Find a Tauri sidecar in its real `.deb` install location: a `yerd*`-named
/// directory under `/usr/lib` (e.g. `/usr/lib/Yerd/yerdd`). Restricted to
/// yerd-named dirs so an unrelated `/usr/lib/<other>/yerd` can't be picked up.
#[cfg(target_os = "linux")]
fn lib_sidecar(name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir("/usr/lib").ok()?;
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .to_lowercase()
                .contains("yerd")
        })
        .map(|e| e.path().join(name))
        .find(|c| c.is_file())
}

/// The resolved `yerdd` path, if present.
pub(crate) fn resolve_yerdd() -> Option<PathBuf> {
    resolve_binary("yerdd")
}

/// Like [`resolve_yerdd`] but **skips the "beside `current_exe`" candidate** —
/// used on the macOS translocated-fallback path, where the sibling `yerdd` lives
/// on an ephemeral AppTranslocation mount that vanishes when torn down (launchd
/// must not be pointed at it). Resolves only from stable install dirs.
#[cfg(target_os = "macos")]
pub(crate) fn resolve_yerdd_stable() -> Option<PathBuf> {
    search_dirs()
        .into_iter()
        .map(|d| d.join("yerdd"))
        .find(|c| c.is_file())
}

/// Is `yerdd` present on disk? With the daemon bundled this is normally true; it
/// stays a command so the frontend can surface a clear error if a build/install
/// is somehow missing the sidecar.
#[tauri::command]
pub fn daemon_installed() -> bool {
    resolve_yerdd().is_some()
}

// ── optional: install the bundled `yerd` CLI on PATH (macOS) ─────────────────
//
// Linux already exposes `yerd` on PATH (the `.deb` postinst symlinks it into
// `/usr/bin`), so this is macOS-only. We symlink the bundled `yerd` into
// `{data}/bin` — the exact dir the `yerd path` rc-block puts on PATH — and shell
// out to the bundled `yerd path install` to manage the rc block (we do NOT depend
// on the `bin/yerd` crate; that would violate the dep-flow rule).

/// `{data}/bin/yerd` — where the CLI symlink lives (matches `yerd path`).
fn cli_symlink_path() -> Result<PathBuf, GuiError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| GuiError::internal(format!("cannot resolve yerd directories: {e}")))?;
    Ok(dirs.data.join("bin").join("yerd"))
}

/// Whether the bundled `yerd` CLI is linked onto PATH.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliPathStatus {
    /// The `{data}/bin/yerd` symlink exists and resolves to a real file.
    pub installed: bool,
    /// The symlink location (for display).
    pub target: String,
}

#[tauri::command]
pub fn cli_path_status() -> Result<CliPathStatus, GuiError> {
    let link = cli_symlink_path()?;
    // A dangling symlink (app moved/removed) reports not-installed so the UI can
    // offer to repair it.
    let installed = link.symlink_metadata().is_ok() && link.exists();
    Ok(CliPathStatus {
        installed,
        target: link.display().to_string(),
    })
}

/// Symlink the bundled `yerd` into `{data}/bin` and ensure that dir is on PATH.
/// macOS-only behaviour — Linux already exposes `yerd` on PATH via the `.deb`.
#[tauri::command]
pub async fn install_cli_to_path() -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    {
        // Refuse when translocated: the symlink would point into an ephemeral
        // `/AppTranslocation/…` mount that disappears.
        if crate::autostart::is_translocated() {
            return Err(GuiError::internal(
                "Move Yerd to your Applications folder first, then install the CLI.",
            ));
        }
        let yerd = resolve_binary("yerd")
            .ok_or_else(|| GuiError::internal("the bundled yerd CLI was not found in the app"))?;
        let link = cli_symlink_path()?;
        // The symlink ops and the `yerd path install` subprocess block; run them
        // off the async runtime so the tray/UI never stalls (mirrors `start`/`stop`).
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = link.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    GuiError::internal(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            // Replace any existing (possibly dangling) link.
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&yerd, &link).map_err(|e| {
                GuiError::internal(format!("cannot link yerd into {}: {e}", link.display()))
            })?;
            // Put `{data}/bin` on PATH via the bundled CLI's own rc-block manager.
            let out = std::process::Command::new(&yerd)
                .args(["path", "install"])
                .output()
                .map_err(|e| {
                    GuiError::internal(format!("could not run `yerd path install`: {e}"))
                })?;
            if !out.status.success() {
                return Err(GuiError::internal(format!(
                    "`yerd path install` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(())
        })
        .await
        .map_err(|e| GuiError::internal(format!("install task failed: {e}")))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(GuiError::internal(
            "The Yerd CLI is already installed on this platform.",
        ))
    }
}

/// Remove the `{data}/bin/yerd` symlink. (Leaves the `yerd path` rc block alone —
/// other yerd shims, e.g. `php`/`composer`, also live in `{data}/bin`.)
#[tauri::command]
pub fn remove_cli_from_path() -> Result<(), GuiError> {
    let link = cli_symlink_path()?;
    match std::fs::remove_file(&link) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(GuiError::internal(format!(
            "cannot remove {}: {e}",
            link.display()
        ))),
    }
}

/// Open **System Settings → General → Login Items** (macOS) so the user can
/// enable the daemon when SMAppService registration is pending approval. No-op
/// on other platforms.
#[tauri::command]
pub fn open_login_items() {
    #[cfg(target_os = "macos")]
    crate::smappservice::open_login_items_settings();
}

// ── start / stop ─────────────────────────────────────────────────────────────

/// Start the daemon. Prefers the per-user service (the single supervisor when
/// available); falls back to a detached `yerdd serve` only when no service
/// manager exists (in which case daemon-at-login is disabled in the UI). The
/// blocking service call runs off the async worker so the tray/UI never stalls.
pub(crate) async fn start(nudge: bool) -> Result<(), GuiError> {
    // Bound the blocking service call so a hung `launchctl`/`systemctl`/
    // SMAppService can't make `start_daemon` never resolve (the frontend awaits
    // it before it even begins polling — another endless-spinner path). The
    // blocking thread isn't cancellable; the timeout only frees the await.
    let join = tokio::task::spawn_blocking(move || crate::autostart::daemon_start(nudge));
    match tokio::time::timeout(std::time::Duration::from_secs(15), join).await {
        Ok(Ok(inner)) => inner,
        Ok(Err(e)) => Err(GuiError::internal(format!("start task failed: {e}"))),
        Err(_) => Err(GuiError::internal(
            "starting the daemon timed out (the service manager did not respond)",
        )),
    }
}

/// Stop the daemon: via the service when one manages it, with a universal
/// SIGTERM-of-the-reported-pid fallback (covers `yerdd serve &`,
/// `cargo run -p yerdd`, etc.). The daemon shuts down gracefully on SIGTERM.
pub(crate) async fn stop() -> Result<(), GuiError> {
    let _ = tokio::task::spawn_blocking(crate::autostart::daemon_stop).await;
    if let Some(pid) = running_pid().await {
        sigterm(pid);
    }
    Ok(())
}

/// Start the daemon. `nudge` (macOS) controls whether a `requiresApproval`
/// SMAppService state opens Login Items — onboarding passes `false` so it opens
/// at most once across the daemon + GUI enables; the General-tab button uses
/// `true`.
#[tauri::command]
pub async fn start_daemon(nudge: bool) -> Result<(), GuiError> {
    start(nudge).await
}

#[tauri::command]
pub async fn stop_daemon() -> Result<(), GuiError> {
    stop().await
}

/// The running daemon's pid via a `status` IPC, or `None` if unreachable.
async fn running_pid() -> Option<u32> {
    // Bounded: this runs in the stop path; a wedged daemon mustn't hang it.
    match crate::ipc::exchange_timeout(
        &yerd_ipc::Request::Status,
        std::time::Duration::from_secs(5),
    )
    .await
    {
        Ok(yerd_ipc::Response::Status { report }) => Some(report.daemon_pid),
        _ => None,
    }
}

/// Send SIGTERM to `pid` (best-effort; an already-dead pid is fine).
fn sigterm(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: `kill` is a libc syscall with no memory effects; sending
        // SIGTERM to a pid cannot invoke UB. A stale pid just returns ESRCH.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

/// Build the detached daemon's stdout/stderr targets: a truncate-on-start
/// `{cache}/yerdd-spawn.log` so a crash *before* the daemon's own tracing log is
/// up (e.g. the tokio runtime-build failure) still leaves a trace. Both fds are
/// `try_clone`d from one `File` so they share an open-file-description (one
/// offset → interleave without clobbering). Best-effort: any failure
/// (unresolvable cache, create/clone error) degrades to `/dev/null` rather than
/// turning logging into a spawn failure.
#[cfg(target_os = "linux")]
fn spawn_log_stdio() -> (std::process::Stdio, std::process::Stdio) {
    use yerd_platform::{ActivePaths, Paths};
    let pair = (|| {
        let dirs = ActivePaths::new().resolve().ok()?;
        std::fs::create_dir_all(&dirs.cache).ok()?;
        let file = std::fs::File::create(dirs.cache.join("yerdd-spawn.log")).ok()?;
        let clone = file.try_clone().ok()?;
        Some((file, clone))
    })();
    match pair {
        Some((out, err)) => (out.into(), err.into()),
        None => (std::process::Stdio::null(), std::process::Stdio::null()),
    }
}

/// Spawn `yerdd serve` detached so it survives the GUI exiting (its own
/// session). stdout/stderr go to `{cache}/yerdd-spawn.log` (or `/dev/null` if
/// that can't be opened). Used only on the no-service-manager path (Linux
/// without systemd `--user`; macOS always has launchd).
#[cfg(target_os = "linux")]
pub(crate) fn spawn_detached() -> Result<(), GuiError> {
    let yerdd = resolve_yerdd().ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
    let (stdout, stderr) = spawn_log_stdio();
    let mut cmd = std::process::Command::new(&yerdd);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `setsid` in the child (pre-exec) detaches it into its own
        // session so it outlives the GUI; it touches no parent memory.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| GuiError::internal(format!("could not start {}: {e}", yerdd.display())))
}

// ── diagnostics ───────────────────────────────────────────────────────────────
//
// When a start attempt fails to connect, the GUI calls `daemon_diagnostics` to
// gather everything that explains *why* — both the ran-and-crashed case (the
// daemon's rolling log tail) and the never-launched cases (the start error,
// translocation, a missing sidecar, pending Login-Items approval), which are the
// likely first-run reports where no daemon log exists yet.

/// A host-side snapshot of daemon-start health. Serialises camelCase for the
/// webview; see `DaemonDiagnostics` in `ipc/types.ts`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonDiagnostics {
    /// The error `start_daemon` threw, passed back from the frontend — the top
    /// signal for "never launched" failures (register / translocation / missing
    /// binary), where the daemon log doesn't exist.
    start_error: Option<String>,
    /// Plain-English cause+fix lines computed from the signals below.
    hints: Vec<String>,
    /// Resolved `yerdd` path, or `None` if the bundled daemon is missing. On
    /// macOS a translocated run reports the *stable* path (or `None`), never the
    /// ephemeral `/AppTranslocation/…` one.
    yerdd_path: Option<String>,
    /// macOS App Translocation (running from a DMG / quarantine mount).
    translocated: bool,
    /// The IPC socket path the GUI connects to.
    socket_path: String,
    /// A real connect+`Status` exchange succeeded — not mere file existence (a
    /// stale socket file can linger after a crash).
    socket_responding: bool,
    /// The connect error from the probe, when it failed.
    last_connect_error: Option<String>,
    /// Which mechanism supervises the daemon here.
    service_manager: String,
    /// The service manager's own status text for the job, truncated.
    service_status: Option<String>,
    /// macOS SMAppService `requiresApproval` — registered but awaiting the user.
    pending_approval: bool,
    /// Newest `{cache}/yerdd.<date>.log`, if any.
    log_path: Option<String>,
    /// Last lines of the daemon's rolling log.
    log_tail: Vec<String>,
    /// Last lines of `{cache}/yerdd-spawn.log` (Linux detached-spawn path).
    spawn_log_tail: Vec<String>,
    /// Last lines of `{cache}/yerd-gui-repair.log` — the GUI's daemon-registration
    /// self-repair trail (macOS upgrade re-registration attempts + outcomes). The
    /// GUI has no `tracing` subscriber, so this file is how the self-repair attempt
    /// (and any technical failure) becomes retrievable via "Copy diagnostics".
    repair_log_tail: Vec<String>,
}

/// Gather daemon-start diagnostics. `start_error` is the message the frontend's
/// `startDaemon` call threw (if any). Probes the socket (async), then does the
/// filesystem/subprocess gathering off the runtime so the UI never stalls.
#[tauri::command]
pub async fn daemon_diagnostics(
    start_error: Option<String>,
) -> Result<DaemonDiagnostics, GuiError> {
    // Bound the probe: a *wedged* daemon (accepting the socket but not replying)
    // would otherwise make this diagnose-a-sick-daemon command hang the UI.
    let probe = crate::ipc::exchange(&yerd_ipc::Request::Status);
    let (socket_responding, last_connect_error) =
        match tokio::time::timeout(std::time::Duration::from_secs(3), probe).await {
            Ok(Ok(_)) => (true, None),
            Ok(Err(e)) => (false, Some(e.message)),
            Err(_) => (
                false,
                Some("daemon did not respond within 3s (it may be wedged)".to_owned()),
            ),
        };
    tokio::task::spawn_blocking(move || {
        build_diagnostics(start_error, socket_responding, last_connect_error)
    })
    .await
    .map_err(|e| GuiError::internal(format!("diagnostics task failed: {e}")))
}

/// Number of trailing log lines surfaced in diagnostics.
const LOG_TAIL_LINES: usize = 80;

fn build_diagnostics(
    start_error: Option<String>,
    socket_responding: bool,
    last_connect_error: Option<String>,
) -> DaemonDiagnostics {
    use yerd_platform::{ActivePaths, Paths};

    let dirs = ActivePaths::new().resolve().ok();
    let socket_path = dirs
        .as_ref()
        .map(|d| d.runtime.join("yerd.sock").display().to_string())
        .unwrap_or_else(|| "<unresolved>".to_owned());
    let cache = dirs.as_ref().map(|d| d.cache.clone());

    let translocated = diag_translocated();
    let yerdd_path = diag_yerdd_path();
    let pending_approval = crate::autostart::daemon_pending_approval();
    let service_manager = crate::autostart::service_manager_label().to_owned();
    let service_status = crate::autostart::service_status_text();

    let log_path = cache
        .as_ref()
        .and_then(|c| newest_rolling_log(c).map(|p| p.display().to_string()));
    let log_tail = log_path
        .as_ref()
        .map(|p| tail_lines(std::path::Path::new(p), LOG_TAIL_LINES))
        .unwrap_or_default();
    let spawn_log_tail = cache
        .as_ref()
        .map(|c| tail_lines(&c.join("yerdd-spawn.log"), LOG_TAIL_LINES))
        .unwrap_or_default();
    let repair_log_tail = cache
        .as_ref()
        .map(|c| tail_lines(&c.join("yerd-gui-repair.log"), LOG_TAIL_LINES))
        .unwrap_or_default();

    let hints = compute_hints(
        pending_approval,
        translocated,
        yerdd_path.is_none(),
        start_error.as_deref(),
        &service_manager,
        &log_tail,
        &spawn_log_tail,
    );

    DaemonDiagnostics {
        start_error,
        hints,
        yerdd_path,
        translocated,
        socket_path,
        socket_responding,
        last_connect_error,
        service_manager,
        service_status,
        pending_approval,
        log_path,
        log_tail,
        spawn_log_tail,
        repair_log_tail,
    }
}

/// "Running from a non-installed location" flag for diagnostics (always false
/// off-macOS). True for App Translocation *and* for launching straight from a
/// mounted disk image (`/Volumes/…`): both make SMAppService registration fail,
/// and both are fixed by moving Yerd to /Applications — so the same
/// "move to Applications" hint applies. This drives only the hint, not the
/// registration gating, so a legitimate read-write external-drive install still
/// registers normally.
fn diag_translocated() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::autostart::is_translocated()
            || std::env::current_exe()
                .map(|p| p.to_string_lossy().contains("/Volumes/"))
                .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The `yerdd` path to report. On a translocated macOS run, resolve only from
/// stable install dirs (the beside-exe candidate is an ephemeral mount that
/// would mislead "installed at …"); otherwise the normal resolver.
fn diag_yerdd_path() -> Option<String> {
    #[cfg(target_os = "macos")]
    if crate::autostart::is_translocated() {
        return resolve_yerdd_stable().map(|p| p.display().to_string());
    }
    resolve_yerdd().map(|p| p.display().to_string())
}

/// Newest `yerdd.<date>.log` under `dir` (the daily rolling appender's output).
/// Matches `yerdd.` + `.log` so it never picks up `yerdd-spawn.log`. Chooses by
/// modification time; tolerates a missing/unreadable dir.
fn newest_rolling_log(dir: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("yerdd.") && name.ends_with(".log")) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// Last `n` lines of `path`. Best-effort: a missing/unreadable file → empty.
/// Bounded inputs (daily rotation + truncate-on-start spawn log) make reading
/// the whole file acceptable.
fn tail_lines(path: &std::path::Path, n: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| (*s).to_owned()).collect()
}

/// Map the gathered signals to one actionable plain-English line each.
fn compute_hints(
    pending_approval: bool,
    translocated: bool,
    yerdd_missing: bool,
    start_error: Option<&str>,
    service_manager: &str,
    log_tail: &[String],
    spawn_log_tail: &[String],
) -> Vec<String> {
    let mut hints = Vec::new();
    if pending_approval {
        hints.push(
            "Approve Yerd under System Settings → Login Items; it'll connect automatically."
                .to_owned(),
        );
    }
    if translocated {
        hints.push(
            "Yerd is running from a temporary location. Move Yerd.app to your Applications \
             folder, then try again."
                .to_owned(),
        );
    }
    if yerdd_missing {
        hints.push("The bundled daemon (yerdd) wasn't found — reinstall Yerd.".to_owned());
    }
    // Scan both logs for known fatal startup signals.
    let logs = log_tail
        .iter()
        .chain(spawn_log_tail.iter())
        .map(|l| l.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    // Key only on the precise `dns_port` token the DNS-bind error logs
    // (startup.rs). A bare "address in use" would also match the *non-fatal*
    // mail-port conflict or an HTTP-port conflict and wrongly blame DNS.
    if logs.contains("dns_port") {
        hints.push(
            "Another service is holding the DNS port. Change `dns_port` in yerd.toml or stop \
             the conflicting resolver."
                .to_owned(),
        );
    }
    if logs.contains("already running") {
        hints.push("Another Yerd daemon is already running.".to_owned());
    }
    // A running daemon older than the config it's reading (e.g. an upgrade left a
    // stale background registration). The `ConfigError::UnsupportedVersion` text.
    if logs.contains("incompatible with supported version") {
        hints.push(
            "The background daemon is running an older version of Yerd and can't read the \
             current config — it may not have finished upgrading. Re-register it (toggle the \
             daemon login item off then on in Settings) or remove any old Yerd.app copies."
                .to_owned(),
        );
    }
    if service_manager == "detached spawn" {
        hints.push(
            "systemd --user wasn't detected in this session, so the daemon was started \
             detached and won't run at login."
                .to_owned(),
        );
    }
    if let Some(err) = start_error {
        hints.push(format!("Start error: {err}"));
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::compute_hints;

    #[test]
    fn version_skew_log_yields_reregister_hint() {
        // The `ConfigError::UnsupportedVersion` Display line in the daemon log.
        let log = vec![
            "ERROR yerdd: yerdd exiting with error error=config: config schema version 7 is \
             incompatible with supported version 6"
                .to_owned(),
        ];
        let hints = compute_hints(false, false, false, None, "launchd", &log, &[]);
        assert!(
            hints.iter().any(|h| h.contains("older version of Yerd")),
            "expected a re-register hint, got {hints:?}"
        );
    }

    #[test]
    fn no_version_skew_hint_when_logs_clean() {
        let hints = compute_hints(false, false, false, None, "launchd", &[], &[]);
        assert!(!hints.iter().any(|h| h.contains("older version of Yerd")));
    }
}
