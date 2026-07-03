//! Autostart + the per-user daemon service mechanism.
//!
//! Two concerns:
//! - **Daemon service** (start/stop/enable-at-login): systemd `--user` on Linux,
//!   launchd LaunchAgent on macOS. The service manager, when available, is the
//!   *single supervisor* - `start` goes through it so a later autostart-enable
//!   can't end up with a second, competing daemon. Detached spawn is used only
//!   when no service manager exists (then daemon-at-login is unsupported).
//! - **GUI autostart**: the app's own login entry. On a bundled, non-translocated
//!   macOS build this is `SMAppService.mainApp` (the "Open at Login" entry,
//!   attributed to "Yerd"); elsewhere - and on dev/translocated macOS - it falls
//!   back to `tauri-plugin-autostart`. A legacy loose `Yerd.plist` from the
//!   plugin path is migrated to SMAppService on first launch
//!   (`migrate_gui_login_if_needed`). The "start minimized" preference lives in
//!   a Rust-readable settings file (no launch arg survives `mainApp`; `main`'s
//!   `launch_probe` detects a login launch instead).
//!
//! Everything is host-side and threads failures through [`GuiError`].

use std::path::PathBuf;
use std::process::Command;

use tauri_plugin_autostart::ManagerExt as _;

use crate::error::GuiError;

// ── GUI settings file (Rust-readable, beside yerd.toml) ──────────────────────

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct GuiSettings {
    /// User intent for "start the daemon at login" (the OS mechanism is applied
    /// on toggle; this is the reliable, cross-platform source of truth for the
    /// switch's shown state).
    #[serde(default)]
    daemon_autostart: bool,
    /// "Start the GUI minimized (hidden to tray)" - read by `main`'s `setup()`
    /// before the webview/localStorage exists, hence a file not localStorage.
    #[serde(default)]
    gui_minimized: bool,
    /// Whether the first-run welcome journey has been completed at least once.
    /// `#[serde(default)]` is mandatory: an existing `gui-settings.json` written
    /// before this field existed must still deserialize (else `load_settings`
    /// silently resets every preference to its default).
    #[serde(default)]
    onboarding_complete: bool,
    /// macOS one-shot: the legacy loose `Yerd.plist` GUI login item has been
    /// migrated to `SMAppService.mainApp` (so it shows as "Yerd" under Open at
    /// Login, not the Developer-ID name). Guards `migrate_gui_login_if_needed` so
    /// it runs once and never re-enables login for a user who later turned it off.
    #[serde(default)]
    gui_login_migrated: bool,
    /// macOS: the daemon version (= GUI version) that last *successfully*
    /// (re)registered the SMAppService daemon agent. Drives the upgrade
    /// self-repair (re-register from the new bundle when this advances) and the
    /// directional guard (an older GUI must never reconfigure a newer daemon).
    /// `#[serde(default)]` - additive; absent in pre-existing settings files.
    #[serde(default)]
    daemon_registered_version: Option<String>,
    /// macOS: set when this (older) GUI found a *newer* registered daemon and
    /// refused to downgrade it (carries the registered version); drives an
    /// Overview banner. Cleared once the versions agree again.
    #[serde(default)]
    daemon_version_conflict: Option<String>,
}

/// Outcome of comparing the running GUI/daemon version against the version that
/// last registered the daemon. Pure + unit-tested; the macOS reconcile acts on
/// it. (Only the macOS reconcile calls `decide`, so it's dead code on other
/// targets - silenced rather than `#[cfg]`-gated so it stays testable on Linux.)
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Decision {
    /// GUI is older than the registered daemon - refuse (carries the registered version).
    Conflict(semver::Version),
    /// The registered daemon already matches this GUI - nothing to do.
    UpToDate,
    /// Version advanced (or unknown) - (re)register the daemon from this bundle.
    Reconcile,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn decide(gui: &semver::Version, stored: Option<&semver::Version>) -> Decision {
    match stored {
        Some(reg) if gui < reg => Decision::Conflict(reg.clone()),
        Some(reg) if gui == reg => Decision::UpToDate,
        _ => Decision::Reconcile,
    }
}

/// The concrete registration action for the macOS self-repair, folded purely from
/// the version [`Decision`] and `phantom`. Pure + unit-tested (kept cross-platform
/// so it builds/tests on Linux). A `phantom` is an `.enabled` (approved, active)
/// registration whose launchd job is nonetheless definitively absent: SMAppService
/// can report `.enabled` while launchd has dropped the job from the user domain
/// (a BTM hiccup, crash, or manual `bootout`), leaving the same version registered.
/// A plain kickstart of a missing job fails, so that case still needs a full
/// re-register even though the version is unchanged. `phantom` is only ever true in
/// the `UpToDate` case; the other decisions ignore it.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum RegAction {
    /// This GUI is older than the registered daemon; refuse (carries reg version).
    Refuse(semver::Version),
    /// Registration matches this GUI and the launchd job is healthy; nothing to do.
    NoOp,
    /// Re-materialise from this bundle: unregister, register_repairing, kickstart.
    Reregister,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn reg_action(decision: Decision, phantom: bool) -> RegAction {
    match decision {
        Decision::Conflict(reg) => RegAction::Refuse(reg),
        Decision::Reconcile => RegAction::Reregister,
        Decision::UpToDate if phantom => RegAction::Reregister,
        Decision::UpToDate => RegAction::NoOp,
    }
}

/// The persisted "this GUI is older than the registered daemon" marker, for the
/// Overview banner. Cross-platform (always `None` off macOS, where it's never set).
#[tauri::command]
pub fn daemon_version_conflict() -> Option<String> {
    load_settings().daemon_version_conflict
}

/// macOS: the daemon version (= GUI version) that last successfully registered
/// the SMAppService agent - surfaced in the diagnostics payload. `None` where the
/// field is unused (other OSes / never registered).
pub(crate) fn daemon_registered_version() -> Option<String> {
    load_settings().daemon_registered_version
}

fn settings_path() -> Result<PathBuf, GuiError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| GuiError::internal(format!("cannot resolve config dir: {e}")))?;
    Ok(dirs.config.join("gui-settings.json"))
}

fn load_settings() -> GuiSettings {
    settings_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_settings(s: &GuiSettings) -> Result<(), GuiError> {
    let path = settings_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GuiError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_vec_pretty(s)
        .map_err(|e| GuiError::internal(format!("serialize settings: {e}")))?;
    std::fs::write(&path, json)
        .map_err(|e| GuiError::internal(format!("cannot write {}: {e}", path.display())))
}

/// Read the persisted "start minimized" preference (used by `main`'s setup).
pub(crate) fn gui_minimized() -> bool {
    load_settings().gui_minimized
}

// ── onboarding / first-run state ─────────────────────────────────────────────

/// First-run decision inputs for the GUI: has the welcome journey been completed,
/// and does this machine already have a Yerd setup?
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    /// The welcome journey has been finished at least once (`gui-settings.json`).
    pub onboarded: bool,
    /// Yerd has been set up before - a config file exists, a PHP version is
    /// installed, or the daemon service is registered. When true (or the daemon
    /// is reachable), the GUI shows the normal app / "Start daemon" screen rather
    /// than the first-run journey.
    pub is_set_up: bool,
}

/// Resolve yerd's platform dirs (config/data/…) from the host environment.
fn resolve_dirs() -> Option<yerd_platform::PlatformDirs> {
    use yerd_platform::{ActivePaths, Paths};
    ActivePaths::new().resolve().ok()
}

/// Whether any PHP version is installed under `{data}/php/php-*`. Dependency-free
/// FS probe (the GUI host doesn't depend on `yerd-php`).
fn any_php_installed(data: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(data.join("php")) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("php-") && e.path().is_dir())
}

/// Whether the daemon service is registered with the OS (independent of whether
/// it's currently running).
fn service_registered() -> bool {
    #[cfg(target_os = "macos")]
    {
        smapp_registered() || plist_path().map(|p| p.exists()).unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        unit_path().map(|p| p.exists()).unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Whether Yerd has been set up on this machine before (see [`SetupState`]).
fn is_set_up() -> bool {
    if let Some(dirs) = resolve_dirs() {
        if dirs.config.join("yerd.toml").is_file() || any_php_installed(&dirs.data) {
            return true;
        }
    }
    service_registered()
}

#[tauri::command]
pub fn setup_state() -> SetupState {
    SetupState {
        onboarded: load_settings().onboarding_complete,
        is_set_up: is_set_up(),
    }
}

/// Mark the first-run welcome journey complete (persisted in `gui-settings.json`).
#[tauri::command]
pub fn mark_onboarded() -> Result<(), GuiError> {
    let mut s = load_settings();
    s.onboarding_complete = true;
    save_settings(&s)
}

// ── command helpers ──────────────────────────────────────────────────────────

fn run_ok(program: &str, args: &[&str]) -> Result<(), GuiError> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| GuiError::internal(format!("`{program}` failed to launch: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(GuiError::internal(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Run a command and return its combined stdout+stderr regardless of exit
/// status, truncated to `max` bytes. Unlike [`run_ok`], this keeps the output
/// of commands that exit non-zero - `systemctl status` / `launchctl print`
/// routinely do, and that text is exactly what diagnostics want. `None` if the
/// command can't be launched at all. Only [`service_status_text`] uses it, on
/// the two platforms with a service manager - gated so a Windows build (where
/// that arm returns `None`) doesn't see it as dead code under `-D warnings`.
///
/// **Bounded.** This runs on a `spawn_blocking` thread in the diagnostics path,
/// but a wedged `systemctl --user`/`launchctl` would still leave the UI's
/// "diagnose" step stuck, so the wait is capped (the child is killed on expiry).
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn capture(program: &str, args: &[&str], max: usize) -> Option<String> {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(format!("(`{program}` did not respond within 4s)"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }

    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_end(&mut out_buf);
    }
    if let Some(mut se) = child.stderr.take() {
        let _ = se.read_to_end(&mut err_buf);
    }

    let mut s = String::from_utf8_lossy(&out_buf).into_owned();
    let err = String::from_utf8_lossy(&err_buf);
    if !err.trim().is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(&err);
    }
    let s = s.trim().to_string();
    if s.is_empty() {
        return None;
    }
    if s.len() > max {
        let mut t = String::new();
        for ch in s.chars() {
            if t.len() + ch.len_utf8() > max {
                break;
            }
            t.push(ch);
        }
        t.push_str("\n… (truncated)");
        Some(t)
    } else {
        Some(s)
    }
}

/// Human-readable label for the mechanism that supervises the daemon on this
/// host - surfaced in diagnostics so a user/support can see which path was used.
pub(crate) fn service_manager_label() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            "systemd --user"
        } else {
            "detached spawn"
        }
    }
    #[cfg(target_os = "macos")]
    {
        if use_smappservice() {
            "launchd (SMAppService)"
        } else {
            "launchd (loose)"
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "none"
    }
}

/// The service manager's own status text for the daemon job (`systemctl --user
/// status yerd` / `launchctl print gui/{uid}/dev.yerd.daemon`), truncated.
/// `None` when no service manager applies or the query produced nothing.
pub(crate) fn service_status_text() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        capture(
            "systemctl",
            &["--user", "status", "yerd", "--no-pager"],
            4096,
        )
    }
    #[cfg(target_os = "macos")]
    {
        capture("launchctl", &["print", &service_target()], 4096)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// ── service-manager availability ─────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn systemd_user_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether a per-user service manager is available (launchd on macOS always is;
/// systemd `--user` on Linux must be probed). When false, the daemon-at-login
/// toggle is unsupported and start/stop fall back to detached spawn + SIGTERM.
pub(crate) fn manager_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        systemd_user_available()
    }
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

// ── Linux: systemd --user unit ───────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn unit_path() -> Result<PathBuf, GuiError> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| crate::daemon::home_dir().map(|h| h.join(".config")))
        .ok_or_else(|| GuiError::internal("cannot resolve XDG config dir"))?;
    Ok(base.join("systemd").join("user").join("yerd.service"))
}

/// Render the systemd unit text (resolving the current `yerdd` path). Extracted
/// so [`unit_is_current`] can compare against the on-disk file without
/// duplicating the template.
#[cfg(target_os = "linux")]
fn render_unit() -> Result<String, GuiError> {
    let yerdd = crate::daemon::resolve_yerdd()
        .ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
    Ok(format!(
        "[Unit]\nDescription=Yerd local PHP development daemon\n\n[Service]\nType=simple\nExecStart={} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        yerdd.display()
    ))
}

/// Whether the on-disk unit already matches what we'd write (same `ExecStart`
/// etc.). Drives only the install-vs-start *label* - `write_unit` still runs
/// unconditionally. Best-effort: any error → `false` (treat as needs-install).
#[cfg(target_os = "linux")]
fn unit_is_current() -> bool {
    match (render_unit(), unit_path()) {
        (Ok(rendered), Ok(path)) => std::fs::read_to_string(&path)
            .map(|c| c == rendered)
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn write_unit() -> Result<(), GuiError> {
    let unit = render_unit()?;
    let path = unit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GuiError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, unit)
        .map_err(|e| GuiError::internal(format!("cannot write {}: {e}", path.display())))?;
    let _ = run_ok("systemctl", &["--user", "daemon-reload"]);
    Ok(())
}

// ── macOS: launchd LaunchAgent ───────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn uid() -> u32 {
    // SAFETY: `getuid` is a libc syscall with no preconditions or memory effects.
    unsafe { libc::getuid() }
}

#[cfg(target_os = "macos")]
fn service_target() -> String {
    format!("gui/{}/dev.yerd.daemon", uid())
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf, GuiError> {
    let home = crate::daemon::home_dir().ok_or_else(|| GuiError::internal("HOME is not set"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join("dev.yerd.daemon.plist"))
}

// ── macOS: SMAppService vs the loose-launchd fallback ────────────────────────

/// Whether to manage the daemon via [`crate::smappservice`] (the bundled,
/// non-translocated release path - gives the "Yerd" Login Items entry) rather
/// than the loose `launchctl bootstrap` fallback. False when:
/// - `YERD_NO_AUTO_DAEMON` is set (the CI launch smoke test → zero SMAppService
///   calls), or
/// - we're not running from an `.app` bundle (`cargo run` dev builds), or
/// - the app is **translocated** (run from a DMG/Downloads - `register()` would
///   fail on the unstable path), or
/// - the embedded agent plist is absent (a plain `tauri build` with no bundle
///   overlay → degrade gracefully instead of erroring with `notFound`).
#[cfg(target_os = "macos")]
fn use_smappservice() -> bool {
    if std::env::var_os("YERD_NO_AUTO_DAEMON").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.to_string_lossy();
    let bundled = exe.contains(".app/Contents/MacOS/");
    bundled && !is_translocated() && embedded_plist_path().is_some_and(|p| p.is_file())
}

/// True if our own executable is under an App Translocation mount (Gatekeeper
/// runs un-quarantined apps from a randomized read-only `/AppTranslocation/…`
/// path until the user moves them to /Applications).
#[cfg(target_os = "macos")]
pub(crate) fn is_translocated() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains("/AppTranslocation/"))
        .unwrap_or(false)
}

/// Path to the agent plist embedded in our own bundle
/// (`Yerd.app/Contents/Library/LaunchAgents/dev.yerd.daemon.plist`), derived
/// from `current_exe` at `…/Contents/MacOS/yerd-gui`.
#[cfg(target_os = "macos")]
fn embedded_plist_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    Some(contents.join("Library/LaunchAgents/dev.yerd.daemon.plist"))
}

/// Whether the daemon is currently registered (or pending approval) via
/// SMAppService. Read-only.
#[cfg(target_os = "macos")]
fn smapp_registered() -> bool {
    crate::smappservice::status(crate::smappservice::Service::Daemon)
        .map(crate::smappservice::status_means_registered)
        .unwrap_or(false)
}

/// Whether SMAppService reports the daemon exactly `.enabled` (approved and
/// active). Unlike [`smapp_registered`], this excludes `.requiresApproval`: a
/// not-yet-approved agent has no loaded launchd job by design, so it must not be
/// mistaken for a phantom (see [`reg_action`]) and re-registered on every launch.
/// Read-only.
#[cfg(target_os = "macos")]
fn smapp_status_enabled() -> bool {
    crate::smappservice::status(crate::smappservice::Service::Daemon)
        .map(|s| s == crate::smappservice::STATUS_ENABLED)
        .unwrap_or(false)
}

/// Migrate away from a prior release's **loose** LaunchAgent before registering
/// via SMAppService. The loose agent and the SMAppService agent share the Label
/// `dev.yerd.daemon`, so an unconditional teardown could boot out the *live*
/// registered agent - therefore this acts **only when the loose plist file
/// actually exists** on disk (the SMAppService agent has no file there). Without
/// it, an upgrade would leave the old `RunAtLoad` loose agent competing for the
/// IPC socket/ports. Best-effort throughout.
#[cfg(target_os = "macos")]
fn cleanup_legacy() {
    let Ok(path) = plist_path() else {
        return;
    };
    if !path.exists() {
        return;
    }
    let target = service_target();
    let _ = run_ok("launchctl", &["bootout", &target]);
    let _ = std::fs::remove_file(&path);
    let _ = run_ok("launchctl", &["enable", &target]);
}

/// After a `register()`, if macOS is waiting for the user to approve the item in
/// Login Items, take them there. Best-effort.
#[cfg(target_os = "macos")]
fn nudge_if_requires_approval() {
    let pending = crate::smappservice::status(crate::smappservice::Service::Daemon)
        .map(|s| s == crate::smappservice::STATUS_REQUIRES_APPROVAL)
        .unwrap_or(false);
    if pending {
        crate::smappservice::open_login_items_settings();
    }
}

// ── macOS: version-stamped daemon registration self-repair ───────────────────
//
// Both a manual in-place app upgrade and the automated self-update replace the
// whole bundle and relaunch the GUI, but neither re-registers the daemon - so
// launchd keeps running the *old* `yerdd` from the stale BTM entry. `setup_app`
// and `daemon_start` run `ensure_daemon_registration` on every launch: when the
// app version has advanced it forces a fresh registration (re-pointing launchd
// at the new bundle); an *older* GUI against a *newer* registered daemon is
// refused outright. All SMAppService mutations are serialized by this lock.

#[cfg(target_os = "macos")]
static DAEMON_REG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// This GUI's version (= the bundled `yerdd` version; both `version.workspace`).
#[cfg(target_os = "macos")]
fn gui_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

/// Run `program args` discarding its output and return whether it exited
/// successfully, bounded by `secs` (kill + `None` on timeout). The bounded
/// sibling of [`capture`] for callers that only need the exit-status predicate -
/// so a wedged `launchctl` can't hang the launch path (this runs from
/// startup-repair and daemon start).
#[cfg(target_os = "macos")]
fn bounded_status(program: &str, args: &[&str], secs: u64) -> Option<bool> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st.success()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
}

/// Whether launchd currently has the daemon job - by **exit status** (a missing
/// job makes `launchctl print` exit non-zero). Unlike `service_status_text()`,
/// which returns `Some("Could not find service…")` for a missing job, this is a
/// true predicate: `true` for an upgrade victim (job exists), `false` for a
/// never-registered fresh user. Bounded so a wedged `launchctl` can't hang the
/// launch path (timeout → `false`: treat an unresponsive launchctl as "no job",
/// which skips re-registration rather than hanging).
#[cfg(target_os = "macos")]
fn daemon_launchd_job_exists() -> bool {
    bounded_status("launchctl", &["print", &service_target()], 4).unwrap_or(false)
}

/// Whether launchd DEFINITIVELY has no job for the daemon - `launchctl print` ran
/// and exited non-zero (`Some(false)`). A timeout yields `None` → `false` here
/// (unknown, not "missing"), so a wedged launchctl never triggers a needless
/// re-register. This is the strict inverse of [`daemon_launchd_job_exists`],
/// which folds a timeout into "no job" because its caller wants the conservative
/// default; the self-repair heal wants the opposite conservatism.
#[cfg(target_os = "macos")]
fn daemon_launchd_job_missing() -> bool {
    bounded_status("launchctl", &["print", &service_target()], 4) == Some(false)
}

/// Append a line to `{cache}/yerd-gui-repair.log`. The GUI has no `tracing`
/// subscriber and bundled-`.app` stderr isn't retrievable, so the self-repair
/// trail goes to a file that `daemon_diagnostics` tails (and "Copy diagnostics"
/// includes). Best-effort; capped so it can't grow unbounded.
#[cfg(target_os = "macos")]
fn repair_log(line: &str) {
    use std::io::Write as _;
    use yerd_platform::{ActivePaths, Paths};
    let Ok(dirs) = ActivePaths::new().resolve() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dirs.cache);
    let path = dirs.cache.join("yerd-gui-repair.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 64 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Re-assert the daemon registration from the *current* bundle when the app
/// version has advanced (in-place or automated upgrade), and refuse an older GUI
/// reconfiguring a newer daemon. Run from `setup_app` (covers both upgrade kinds)
/// and `daemon_start`. Serialized by [`DAEMON_REG_LOCK`].
///
/// Returns `Ok(true)` when it re-registered the daemon from this bundle (upgrade /
/// phantom re-register) and therefore already issued a `kickstart -k`; `Ok(false)`
/// when nothing needed doing. The re-register itself starts the daemon via the
/// agent's `RunAtLoad`, so the accompanying kickstart is best-effort (its result is
/// deliberately discarded). The daemon-start plan reads this flag to keep a
/// single-kickstart-per-start invariant (see [`plan_start`]).
#[cfg(target_os = "macos")]
pub(crate) fn ensure_daemon_registration() -> Result<bool, GuiError> {
    if !use_smappservice() {
        return Ok(false);
    }
    let _guard = DAEMON_REG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let gui = gui_version();
    let mut s = load_settings();

    let has_registration = s.daemon_autostart || smapp_registered() || daemon_launchd_job_exists();
    if !has_registration {
        if s.daemon_registered_version.is_some() || s.daemon_version_conflict.is_some() {
            s.daemon_registered_version = None;
            s.daemon_version_conflict = None;
            let _ = save_settings(&s);
        }
        return Ok(false);
    }

    let stored = s
        .daemon_registered_version
        .as_deref()
        .and_then(|v| semver::Version::parse(v).ok());

    let decision = decide(&gui, stored.as_ref());
    let phantom = matches!(decision, Decision::UpToDate)
        && smapp_status_enabled()
        && daemon_launchd_job_missing();

    match reg_action(decision, phantom) {
        RegAction::Refuse(reg) => {
            repair_log(&format!(
                "version conflict: GUI {gui} < registered daemon {reg}; refusing to reconfigure/downgrade"
            ));
            let reg_s = reg.to_string();
            if s.daemon_version_conflict.as_deref() != Some(reg_s.as_str()) {
                s.daemon_version_conflict = Some(reg_s);
                let _ = save_settings(&s);
            }
            return Err(GuiError::internal(format!(
                "This Yerd ({gui}) is older than the registered background daemon ({reg}). \
                 Refusing to reconfigure or downgrade it — install Yerd {reg} or newer."
            )));
        }
        RegAction::NoOp => {
            if s.daemon_version_conflict.is_some() {
                s.daemon_version_conflict = None;
                let _ = save_settings(&s);
            }
            return Ok(false);
        }
        RegAction::Reregister => {}
    }

    let reason = if phantom {
        "launchd job missing despite enabled registration"
    } else {
        "version advanced"
    };
    repair_log(&format!(
        "self-repair: re-registering daemon for {gui} (was {stored:?}, {reason})"
    ));
    let _ = crate::smappservice::unregister(crate::smappservice::Service::Daemon);
    match crate::smappservice::register_repairing(crate::smappservice::Service::Daemon) {
        Ok(()) => {
            let _ = run_ok("launchctl", &["kickstart", "-k", &service_target()]);
            let mut s = load_settings();
            s.daemon_registered_version = Some(gui.to_string());
            s.daemon_version_conflict = None;
            let _ = save_settings(&s);
            nudge_if_requires_approval();
            repair_log(&format!("self-repair: OK, daemon registered for {gui}"));
            Ok(true)
        }
        Err(e) => {
            repair_log(&format!("self-repair FAILED: {}", e.message));
            Err(e)
        }
    }
}

/// Register the SMAppService agent if not already on (idempotent), migrating any
/// loose legacy agent first, then nudge for approval if needed. `nudge = false`
/// suppresses the System-Settings open so the onboarding flow (which enables the
/// daemon *and* the GUI) doesn't open Login Items more than once.
#[cfg(target_os = "macos")]
fn smapp_enable(nudge: bool) -> Result<(), GuiError> {
    let _guard = DAEMON_REG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if smapp_registered() {
        if nudge {
            nudge_if_requires_approval();
        }
        return Ok(());
    }
    cleanup_legacy();
    crate::smappservice::register_repairing(crate::smappservice::Service::Daemon)?;
    {
        let mut s = load_settings();
        s.daemon_registered_version = Some(gui_version().to_string());
        let _ = save_settings(&s);
    }
    if nudge {
        nudge_if_requires_approval();
    }
    Ok(())
}

// ── macOS: GUI login item via SMAppService.mainApp ───────────────────────────
//
// The GUI "launch at login" used to ride `tauri-plugin-autostart` in LaunchAgent
// mode, which writes a *loose* `~/Library/LaunchAgents/Yerd.plist`. macOS files
// loose agents under the signing identity's name (an individual's legal name),
// not "Yerd". Registering the main app via `SMAppService.mainApp` puts it under
// **Login Items → Open at Login** attributed to "Yerd". `mainApp` uses the app's
// own `Info.plist`, so there is no embedded plist and no bundle-config change.

/// The loose `~/Library/LaunchAgents/Yerd.plist` that `tauri-plugin-autostart`
/// writes (Label `Yerd`). Present only on un-migrated installs.
#[cfg(target_os = "macos")]
fn gui_loose_plist_path() -> Result<PathBuf, GuiError> {
    let home = crate::daemon::home_dir().ok_or_else(|| GuiError::internal("HOME is not set"))?;
    Ok(home.join("Library").join("LaunchAgents").join("Yerd.plist"))
}

/// launchctl service target for the loose plist (its Label is `Yerd`).
#[cfg(target_os = "macos")]
fn gui_loose_service_target() -> String {
    format!("gui/{}/Yerd", uid())
}

/// Whether to manage GUI login via SMAppService (vs the `tauri-plugin-autostart`
/// fallback). Same gates as the daemon's [`use_smappservice`] *except* no
/// embedded-plist check - `mainApp` registers the app's own `Info.plist`.
#[cfg(target_os = "macos")]
fn gui_use_smappservice() -> bool {
    if std::env::var_os("YERD_NO_AUTO_DAEMON").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe.to_string_lossy().contains(".app/Contents/MacOS/") && !is_translocated()
}

/// Whether the GUI main-app login item is registered (or pending approval).
#[cfg(target_os = "macos")]
fn gui_smapp_registered() -> bool {
    crate::smappservice::status(crate::smappservice::Service::MainApp)
        .map(crate::smappservice::status_means_registered)
        .unwrap_or(false)
}

/// macOS only: the GUI login item is registered but awaiting the user's approval
/// in Login Items. Always false on the fallback path / other OSes.
fn gui_pending_approval() -> bool {
    #[cfg(target_os = "macos")]
    if gui_use_smappservice() {
        return crate::smappservice::status(crate::smappservice::Service::MainApp)
            .map(|s| s == crate::smappservice::STATUS_REQUIRES_APPROVAL)
            .unwrap_or(false);
    }
    false
}

/// Tear down the loose `tauri-plugin-autostart` login item (`Yerd.plist`).
/// Best-effort, and a no-op when the file is absent. Safe even with a live
/// `mainApp` registration: that item is keyed on the app bundle, not a launchd
/// Label, so booting out `gui/{uid}/Yerd` cannot affect it.
#[cfg(target_os = "macos")]
fn gui_cleanup_legacy() {
    let Ok(path) = gui_loose_plist_path() else {
        return;
    };
    if !path.exists() {
        return;
    }
    let target = gui_loose_service_target();
    let _ = run_ok("launchctl", &["bootout", &target]);
    let _ = std::fs::remove_file(&path);
    let _ = run_ok("launchctl", &["enable", &target]);
}

/// Register the GUI as a login item via `SMAppService.mainApp` (idempotent).
///
/// Ordering matters: register **first**, then remove the loose `Yerd.plist` only
/// on success - so a failed register leaves the old login item intact rather than
/// silently de-registering the user, and a later attempt can still migrate it.
/// `nudge = false` suppresses the Login-Items open (onboarding opens it once).
#[cfg(target_os = "macos")]
fn gui_smapp_enable(nudge: bool) -> Result<(), GuiError> {
    if gui_smapp_registered() {
        gui_cleanup_legacy();
        if nudge && gui_pending_approval() {
            crate::smappservice::open_login_items_settings();
        }
        return Ok(());
    }
    crate::smappservice::register_repairing(crate::smappservice::Service::MainApp)?;
    gui_cleanup_legacy();
    if nudge && gui_pending_approval() {
        crate::smappservice::open_login_items_settings();
    }
    Ok(())
}

/// Unregister the GUI login item and remove any leftover loose plist.
#[cfg(target_os = "macos")]
fn gui_smapp_disable() -> Result<(), GuiError> {
    if gui_smapp_registered() {
        crate::smappservice::unregister(crate::smappservice::Service::MainApp)?;
    }
    gui_cleanup_legacy();
    Ok(())
}

/// One-time startup migration of an existing loose `Yerd.plist` login item to
/// `SMAppService.mainApp`, so already-"login-on" users stop showing under the
/// Developer-ID name. Flag-guarded (`gui_login_migrated`) so it runs once and
/// won't re-enable login for a user who later turned it off. Silent - never
/// opens System Settings at startup (the General-tab banner surfaces approval).
/// The one-shot flag is set only on success, so a failed register retries next
/// launch with the loose plist still in place.
#[cfg(target_os = "macos")]
pub(crate) fn migrate_gui_login_if_needed() {
    if !gui_use_smappservice() {
        return;
    }
    let mut s = load_settings();
    if s.gui_login_migrated {
        return;
    }
    let loose = gui_loose_plist_path().map(|p| p.exists()).unwrap_or(false);
    if loose {
        if gui_smapp_enable(false).is_ok() {
            s.gui_login_migrated = true;
            let _ = save_settings(&s);
        }
    } else {
        s.gui_login_migrated = true;
        let _ = save_settings(&s);
    }
}

/// Write the LaunchAgent plist and bootstrap it (idempotent - an already-loaded
/// agent is fine). `RunAtLoad` + `KeepAlive{SuccessfulExit:false}`: it relaunches
/// at login *when enabled* and after a crash, but a clean stop stays stopped.
#[cfg(target_os = "macos")]
fn ensure_bootstrapped() -> Result<(), GuiError> {
    let yerdd = if is_translocated() {
        crate::daemon::resolve_yerdd_stable().ok_or_else(|| {
            GuiError::internal(
                "Yerd is running from a temporary location. Move Yerd.app to your \
                 Applications folder (or install the Yerd CLI) to run the daemon.",
            )
        })?
    } else {
        crate::daemon::resolve_yerdd()
            .ok_or_else(|| GuiError::internal("yerdd is not installed"))?
    };
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GuiError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>dev.yerd.daemon</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string><string>serve</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key>\n  <dict><key>SuccessfulExit</key><false/></dict>\n  <key>ThrottleInterval</key><integer>5</integer>\n</dict>\n</plist>\n",
        xml_escape(&yerdd.display().to_string())
    );
    std::fs::write(&path, plist)
        .map_err(|e| GuiError::internal(format!("cannot write {}: {e}", path.display())))?;
    let _ = run_ok(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{}", uid()),
            &path.to_string_lossy(),
        ],
    );
    Ok(())
}

// ── daemon start / stop / autostart (used by crate::daemon + the commands) ───

/// A phase of "starting the daemon", surfaced to the GUI start button so it can
/// show the current step. Rust emits only the phases that map to work it
/// performs (install / upgrade / start); the frontend owns `running` (the
/// readiness wait) and `idle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartPhase {
    Installing,
    Upgrading,
    Starting,
}

impl StartPhase {
    /// Wire string for the `daemon-start-phase` Tauri event.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            StartPhase::Installing => "installing",
            StartPhase::Upgrading => "upgrading",
            StartPhase::Starting => "starting",
        }
    }

    /// Subject for a phase-named timeout error ("<subject> timed out …").
    pub(crate) fn timed_out_subject(self) -> &'static str {
        match self {
            StartPhase::Installing => "installing the daemon",
            StartPhase::Upgrading => "upgrading the daemon",
            StartPhase::Starting => "starting the daemon",
        }
    }
}

/// One ordered step of the start sequence: a phase label (for the button), its
/// OWN timeout budget, and the blocking work to run. `daemon.rs` emits the phase,
/// then runs `run` inside a `spawn_blocking` bounded by `budget`. The budget is
/// explicit (not derived from the label) so the slow macOS register/reconcile
/// step gets the worst-case allowance even when its label is the optimistic
/// "Starting". Independent per-step budgets replace the former single combined
/// 15 s, so a hung `launchctl`/`systemctl` can't exceed one step's slice.
pub(crate) struct StartStep {
    pub phase: StartPhase,
    pub budget: std::time::Duration,
    pub run: Box<dyn FnOnce() -> Result<(), GuiError> + Send>,
}

/// Writing a unit/plist + service-manager start.
const INSTALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(12);
/// The macOS SMAppService ensure/register step: unregister + register_repairing +
/// kickstart is the slowest single action (XPC), so it gets the largest slice
/// regardless of the optimistic label `reg_phase` chose. macOS-only - the Linux
/// start plan never uses it.
#[cfg(target_os = "macos")]
const REGISTER_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
/// A plain service-manager start/kickstart.
const START_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

/// Map the macOS daemon-registration state to the phase *label* to show. Pure +
/// unit-tested (kept cross-platform so it builds/tests on Linux); the step still
/// calls the real [`ensure_daemon_registration`], which re-derives and acts on
/// the same inputs.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn reg_plan(
    has_registration: bool,
    gui: &semver::Version,
    stored: Option<&semver::Version>,
) -> StartPhase {
    if !has_registration {
        return StartPhase::Installing;
    }
    match stored {
        Some(s) if gui > s => StartPhase::Upgrading,
        _ => StartPhase::Starting,
    }
}

/// Label for the macOS SMAppService register/ensure step (gathers the live
/// registration inputs, then defers to the pure [`reg_plan`]). May block on a
/// `launchctl print` probe - callers run it off the async runtime.
#[cfg(target_os = "macos")]
fn reg_phase() -> StartPhase {
    let gui = gui_version();
    let s = load_settings();
    let has_registration = s.daemon_autostart || smapp_registered() || daemon_launchd_job_exists();
    let stored = s
        .daemon_registered_version
        .as_deref()
        .and_then(|v| semver::Version::parse(v).ok());
    reg_plan(has_registration, &gui, stored.as_ref())
}

/// Build the ordered start steps for this platform. **May block** (the macOS
/// label probes `launchctl`, the Linux label reads the unit file), so
/// `daemon.rs` calls it inside `spawn_blocking`. Each step keeps the same work
/// the former one-shot `daemon_start` did; only the timeout is now per-phase.
///
/// macOS single-kickstart invariant: on the upgrade / phantom re-register path
/// [`ensure_daemon_registration`] already kickstarts the daemon from the fresh
/// bundle, so the Starting step skips its own `kickstart -k` (it reports back via
/// the register step's return). A second kill inside launchd's minimum-runtime
/// window would trip the daemon's `ThrottleInterval`, blocking `kickstart` past
/// `START_BUDGET` and surfacing a false "service manager did not respond" even
/// though the throttled daemon then comes up on its own.
pub(crate) fn plan_start(nudge: bool) -> Result<Vec<StartStep>, GuiError> {
    #[cfg(target_os = "linux")]
    {
        let _ = nudge;
        if systemd_user_available() {
            if unit_is_current() {
                return Ok(vec![StartStep {
                    phase: StartPhase::Starting,
                    budget: INSTALL_BUDGET,
                    run: Box::new(|| {
                        write_unit()?;
                        run_ok("systemctl", &["--user", "start", "yerd"])
                    }),
                }]);
            }
            return Ok(vec![
                StartStep {
                    phase: StartPhase::Installing,
                    budget: INSTALL_BUDGET,
                    run: Box::new(write_unit),
                },
                StartStep {
                    phase: StartPhase::Starting,
                    budget: START_BUDGET,
                    run: Box::new(|| run_ok("systemctl", &["--user", "start", "yerd"])),
                },
            ]);
        }
        Ok(vec![StartStep {
            phase: StartPhase::Starting,
            budget: START_BUDGET,
            run: Box::new(crate::daemon::spawn_detached),
        }])
    }
    #[cfg(target_os = "macos")]
    {
        if use_smappservice() {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;
            let reg_kickstarted = Arc::new(AtomicBool::new(false));
            let reg_kickstarted_setter = Arc::clone(&reg_kickstarted);
            Ok(vec![
                StartStep {
                    phase: reg_phase(),
                    budget: REGISTER_BUDGET,
                    run: Box::new(move || {
                        let kickstart_issued = ensure_daemon_registration()?;
                        smapp_enable(nudge)?;
                        reg_kickstarted_setter.store(kickstart_issued, Ordering::SeqCst);
                        Ok(())
                    }),
                },
                StartStep {
                    phase: StartPhase::Starting,
                    budget: START_BUDGET,
                    run: Box::new(move || {
                        if reg_kickstarted.load(Ordering::SeqCst) {
                            return Ok(());
                        }
                        let _ = run_ok("launchctl", &["kickstart", "-k", &service_target()]);
                        Ok(())
                    }),
                },
            ])
        } else {
            Ok(vec![
                StartStep {
                    phase: StartPhase::Installing,
                    budget: INSTALL_BUDGET,
                    run: Box::new(ensure_bootstrapped),
                },
                StartStep {
                    phase: StartPhase::Starting,
                    budget: START_BUDGET,
                    run: Box::new(|| run_ok("launchctl", &["kickstart", "-k", &service_target()])),
                },
            ])
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = nudge;
        Err(GuiError::internal(
            "starting the daemon is not supported on this platform",
        ))
    }
}

/// Stop the daemon via the service manager (best-effort; the caller adds a
/// universal SIGTERM-of-pid fallback for daemons not under the service).
pub(crate) fn daemon_stop() {
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            let _ = run_ok("systemctl", &["--user", "stop", "yerd"]);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = run_ok("launchctl", &["kill", "SIGTERM", &service_target()]);
    }
}

/// Enable/disable launch at login. `nudge` (macOS) gates the auto-open of Login
/// Items on `requiresApproval` so onboarding can open it just once.
fn daemon_set_login(on: bool, nudge: bool) -> Result<(), GuiError> {
    #[cfg(target_os = "linux")]
    {
        let _ = nudge;
        if !systemd_user_available() {
            return Err(GuiError::internal(
                "systemd --user is unavailable; cannot manage daemon autostart",
            ));
        }
        write_unit()?;
        run_ok(
            "systemctl",
            &["--user", if on { "enable" } else { "disable" }, "yerd"],
        )
    }
    #[cfg(target_os = "macos")]
    {
        if use_smappservice() {
            if on {
                ensure_daemon_registration()?;
                smapp_enable(nudge)
            } else {
                if smapp_registered() {
                    let gui = gui_version();
                    let stored = load_settings()
                        .daemon_registered_version
                        .as_deref()
                        .and_then(|v| semver::Version::parse(v).ok());
                    if let Some(reg) = stored {
                        if gui < reg {
                            return Err(GuiError::internal(format!(
                                "This Yerd ({gui}) is older than the registered background \
                                 daemon ({reg}); refusing to unregister it — install Yerd \
                                 {reg} or newer."
                            )));
                        }
                    }
                    crate::smappservice::unregister(crate::smappservice::Service::Daemon)?;
                }
                cleanup_legacy();
                Ok(())
            }
        } else {
            ensure_bootstrapped()?;
            run_ok(
                "launchctl",
                &[if on { "enable" } else { "disable" }, &service_target()],
            )
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (on, nudge);
        Err(GuiError::internal(
            "daemon autostart is not supported on this platform",
        ))
    }
}

// ── commands ─────────────────────────────────────────────────────────────────

/// Current autostart state for the General tab.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartState {
    /// Daemon-at-login intent (false when unsupported).
    pub daemon: bool,
    /// Whether daemon autostart is even possible (a service manager exists).
    pub daemon_supported: bool,
    /// GUI-at-login: SMAppService.mainApp registration on the bundled macOS path,
    /// else the autostart plugin's state.
    pub gui: bool,
    /// Start-the-GUI-minimized preference.
    pub gui_minimized: bool,
    /// macOS only: the daemon is registered but **waiting for the user to enable
    /// it in System Settings → Login Items** (SMAppService `requiresApproval`).
    /// Drives a first-run banner; always false elsewhere.
    pub daemon_pending_approval: bool,
    /// macOS only: the GUI login item is registered but **waiting for the user to
    /// enable it in System Settings → Login Items** (SMAppService
    /// `requiresApproval`). Drives a banner; always false elsewhere.
    pub gui_pending_approval: bool,
}

/// Current "run daemon at login" state for the General tab. On the macOS
/// SMAppService path this is the live registration status (the toggle *is*
/// registration), plus a **read-only** reconciliation: a leftover loose agent (a
/// translocated first run, or a pre-SMAppService release) reports as on so the
/// UI doesn't lie - the next explicit enable/start runs the safe
/// `cleanup_legacy()`. Linux + the macOS fallback use the stored intent flag.
fn daemon_enabled(settings: &GuiSettings, supported: bool) -> bool {
    #[cfg(target_os = "macos")]
    if use_smappservice() {
        let legacy = plist_path().map(|p| p.exists()).unwrap_or(false);
        return smapp_registered() || legacy;
    }
    supported && settings.daemon_autostart
}

/// macOS SMAppService `requiresApproval` - registered but pending the user's
/// toggle in Login Items. Always false on the fallback path / other OSes.
pub(crate) fn daemon_pending_approval() -> bool {
    #[cfg(target_os = "macos")]
    if use_smappservice() {
        return crate::smappservice::status(crate::smappservice::Service::Daemon)
            .map(|s| s == crate::smappservice::STATUS_REQUIRES_APPROVAL)
            .unwrap_or(false);
    }
    false
}

/// "Run the Yerd app at login" state for the General tab. On the bundled macOS
/// path this is the live SMAppService.mainApp registration plus a read-only
/// reconcile of a leftover loose `Yerd.plist` (so an un-migrated install doesn't
/// read as off); elsewhere it's the autostart plugin's own state.
fn gui_login_enabled(app: &tauri::AppHandle) -> Result<bool, GuiError> {
    #[cfg(target_os = "macos")]
    if gui_use_smappservice() {
        let loose = gui_loose_plist_path().map(|p| p.exists()).unwrap_or(false);
        return Ok(gui_smapp_registered() || loose);
    }
    app.autolaunch()
        .is_enabled()
        .map_err(|e| GuiError::internal(format!("could not query GUI autostart: {e}")))
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<AutostartState, GuiError> {
    let settings = load_settings();
    let supported = manager_available();
    let gui = gui_login_enabled(&app)?;
    Ok(AutostartState {
        daemon: daemon_enabled(&settings, supported),
        daemon_supported: supported,
        gui,
        gui_minimized: settings.gui_minimized,
        daemon_pending_approval: daemon_pending_approval(),
        gui_pending_approval: gui_pending_approval(),
    })
}

#[tauri::command]
pub fn set_autostart_daemon(on: bool, nudge: bool) -> Result<(), GuiError> {
    daemon_set_login(on, nudge)?;
    let mut s = load_settings();
    s.daemon_autostart = on;
    save_settings(&s)
}

#[tauri::command]
pub fn set_autostart_gui(app: tauri::AppHandle, on: bool, nudge: bool) -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    if gui_use_smappservice() {
        return if on {
            gui_smapp_enable(nudge)
        } else {
            gui_smapp_disable()
        };
    }
    let _ = nudge;
    let mgr = app.autolaunch();
    let r = if on { mgr.enable() } else { mgr.disable() };
    r.map_err(|e| GuiError::internal(format!("could not change GUI autostart: {e}")))
}

#[tauri::command]
pub fn set_gui_minimized(on: bool) -> Result<(), GuiError> {
    let mut s = load_settings();
    s.gui_minimized = on;
    save_settings(&s)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{decide, reg_action, reg_plan, Decision, RegAction, StartPhase};
    use semver::Version;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn reg_plan_installs_when_unregistered() {
        assert_eq!(reg_plan(false, &v("2.0.3"), None), StartPhase::Installing);
        assert_eq!(
            reg_plan(false, &v("2.0.3"), Some(&v("2.0.1"))),
            StartPhase::Installing
        );
    }

    #[test]
    fn reg_plan_upgrades_on_version_advance() {
        assert_eq!(
            reg_plan(true, &v("2.0.3"), Some(&v("2.0.2"))),
            StartPhase::Upgrading
        );
    }

    #[test]
    fn reg_plan_starts_when_current_unknown_or_conflicting() {
        assert_eq!(
            reg_plan(true, &v("2.0.3"), Some(&v("2.0.3"))),
            StartPhase::Starting
        );
        assert_eq!(reg_plan(true, &v("2.0.3"), None), StartPhase::Starting);
        assert_eq!(
            reg_plan(true, &v("2.0.2"), Some(&v("2.0.3"))),
            StartPhase::Starting
        );
    }

    #[test]
    fn decide_reconciles_on_advance_or_unknown() {
        assert_eq!(decide(&v("2.0.3"), None), Decision::Reconcile);
        assert_eq!(decide(&v("2.0.3"), Some(&v("2.0.2"))), Decision::Reconcile);
        assert_eq!(
            decide(&v("2.0.2"), Some(&v("2.0.2-rc.6"))),
            Decision::Reconcile
        );
    }

    #[test]
    fn decide_is_up_to_date_on_equal() {
        assert_eq!(decide(&v("2.0.3"), Some(&v("2.0.3"))), Decision::UpToDate);
    }

    #[test]
    fn decide_conflicts_when_gui_is_older() {
        assert_eq!(
            decide(&v("2.0.2"), Some(&v("2.0.3"))),
            Decision::Conflict(v("2.0.3"))
        );
        assert_eq!(
            decide(&v("2.0.2-rc.6"), Some(&v("2.0.2-rc.7"))),
            Decision::Conflict(v("2.0.2-rc.7"))
        );
    }

    #[test]
    fn reg_action_covers_version_and_phantom() {
        let cases = [
            (
                Decision::Conflict(v("2.1.0")),
                false,
                RegAction::Refuse(v("2.1.0")),
            ),
            (
                Decision::Conflict(v("2.1.0")),
                true,
                RegAction::Refuse(v("2.1.0")),
            ),
            (Decision::Reconcile, false, RegAction::Reregister),
            (Decision::Reconcile, true, RegAction::Reregister),
            (Decision::UpToDate, false, RegAction::NoOp),
            (Decision::UpToDate, true, RegAction::Reregister),
        ];
        for (decision, phantom, expected) in cases {
            assert_eq!(reg_action(decision, phantom), expected);
        }
    }
}
