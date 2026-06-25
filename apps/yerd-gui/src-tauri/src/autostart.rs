//! Autostart + the per-user daemon service mechanism.
//!
//! Two concerns:
//! - **Daemon service** (start/stop/enable-at-login): systemd `--user` on Linux,
//!   launchd LaunchAgent on macOS. The service manager, when available, is the
//!   *single supervisor* — `start` goes through it so a later autostart-enable
//!   can't end up with a second, competing daemon. Detached spawn is used only
//!   when no service manager exists (then daemon-at-login is unsupported).
//! - **GUI autostart**: `tauri-plugin-autostart` (the app's own login entry).
//!   The "start minimized" preference can't ride on launch args (the plugin
//!   fixes those at init), so it lives in a tiny Rust-readable settings file.
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
    /// "Start the GUI minimized (hidden to tray)" — read by `main`'s `setup()`
    /// before the webview/localStorage exists, hence a file not localStorage.
    #[serde(default)]
    gui_minimized: bool,
    /// Whether the first-run welcome journey has been completed at least once.
    /// `#[serde(default)]` is mandatory: an existing `gui-settings.json` written
    /// before this field existed must still deserialize (else `load_settings`
    /// silently resets every preference to its default).
    #[serde(default)]
    onboarding_complete: bool,
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
    /// Yerd has been set up before — a config file exists, a PHP version is
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

// ── service-manager availability ─────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn systemd_user_available() -> bool {
    // `show-environment` is read-only and exits 0 only with a live `--user`
    // bus/manager — unlike `is-system-running`, which is non-zero on a
    // healthy-but-`degraded` system and would false-negative.
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

#[cfg(target_os = "linux")]
fn write_unit() -> Result<(), GuiError> {
    let yerdd = crate::daemon::resolve_yerdd()
        .ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
    let path = unit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GuiError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    let unit = format!(
        "[Unit]\nDescription=Yerd local PHP development daemon\n\n[Service]\nType=simple\nExecStart={} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        yerdd.display()
    );
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
/// non-translocated release path — gives the "Yerd" Login Items entry) rather
/// than the loose `launchctl bootstrap` fallback. False when:
/// - `YERD_NO_AUTO_DAEMON` is set (the CI launch smoke test → zero SMAppService
///   calls), or
/// - we're not running from an `.app` bundle (`cargo run` dev builds), or
/// - the app is **translocated** (run from a DMG/Downloads — `register()` would
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
    // exe = …/Contents/MacOS/yerd-gui → Contents = parent().parent()
    let contents = exe.parent()?.parent()?;
    Some(contents.join("Library/LaunchAgents/dev.yerd.daemon.plist"))
}

/// Whether the daemon is currently registered (or pending approval) via
/// SMAppService. Read-only.
#[cfg(target_os = "macos")]
fn smapp_registered() -> bool {
    crate::smappservice::status()
        .map(crate::smappservice::status_means_registered)
        .unwrap_or(false)
}

/// Migrate away from a prior release's **loose** LaunchAgent before registering
/// via SMAppService. The loose agent and the SMAppService agent share the Label
/// `dev.yerd.daemon`, so an unconditional teardown could boot out the *live*
/// registered agent — therefore this acts **only when the loose plist file
/// actually exists** on disk (the SMAppService agent has no file there). Without
/// it, an upgrade would leave the old `RunAtLoad` loose agent competing for the
/// IPC socket/ports. Best-effort throughout.
#[cfg(target_os = "macos")]
fn cleanup_legacy() {
    let Ok(path) = plist_path() else {
        return;
    };
    if !path.exists() {
        return; // No loose agent → nothing to migrate; don't touch the shared
                // Label (the registered SMAppService agent owns it).
    }
    let target = service_target();
    let _ = run_ok("launchctl", &["bootout", &target]);
    let _ = std::fs::remove_file(&path);
    // Clear any stale *disabled* override an old fallback toggle may have set.
    let _ = run_ok("launchctl", &["enable", &target]);
}

/// After a `register()`, if macOS is waiting for the user to approve the item in
/// Login Items, take them there. Best-effort.
#[cfg(target_os = "macos")]
fn nudge_if_requires_approval() {
    let pending = crate::smappservice::status()
        .map(|s| s == crate::smappservice::STATUS_REQUIRES_APPROVAL)
        .unwrap_or(false);
    if pending {
        crate::smappservice::open_login_items_settings();
    }
}

/// Register the SMAppService agent if not already on (idempotent), migrating any
/// loose legacy agent first, then nudge for approval if needed.
#[cfg(target_os = "macos")]
fn smapp_enable() -> Result<(), GuiError> {
    if smapp_registered() {
        return Ok(()); // already on — don't cleanup/re-register a live agent.
    }
    cleanup_legacy();
    crate::smappservice::register()?;
    nudge_if_requires_approval();
    Ok(())
}

/// Write the LaunchAgent plist and bootstrap it (idempotent — an already-loaded
/// agent is fine). `RunAtLoad` + `KeepAlive{SuccessfulExit:false}`: it relaunches
/// at login *when enabled* and after a crash, but a clean stop stays stopped.
#[cfg(target_os = "macos")]
fn ensure_bootstrapped() -> Result<(), GuiError> {
    // On the translocated-fallback path, `current_exe`'s sibling `yerdd` lives on
    // an ephemeral AppTranslocation mount that vanishes when torn down — launchd
    // must not point at it. Resolve from a stable location only; if there's none,
    // refuse and guide the user to /Applications rather than bootstrap a doomed
    // agent.
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
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>dev.yerd.daemon</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string><string>serve</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key>\n  <dict><key>SuccessfulExit</key><false/></dict>\n</dict>\n</plist>\n",
        xml_escape(&yerdd.display().to_string())
    );
    std::fs::write(&path, plist)
        .map_err(|e| GuiError::internal(format!("cannot write {}: {e}", path.display())))?;
    // Bootstrap; ignore failure (most commonly "already bootstrapped").
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

/// Start the daemon via the service manager, or a detached spawn when none.
pub(crate) fn daemon_start() -> Result<(), GuiError> {
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            write_unit()?;
            return run_ok("systemctl", &["--user", "start", "yerd"]);
        }
        crate::daemon::spawn_detached()
    }
    #[cfg(target_os = "macos")]
    {
        if use_smappservice() {
            // Unified model: ensure registered (which RunAtLoad-starts it), then
            // kickstart for a fresh start even if it was already up. kickstart is
            // best-effort — when status is `requiresApproval` the job isn't loaded
            // yet, and that's fine (the user was sent to Login Items).
            smapp_enable()?;
            let _ = run_ok("launchctl", &["kickstart", "-k", &service_target()]);
            Ok(())
        } else {
            ensure_bootstrapped()?;
            run_ok("launchctl", &["kickstart", "-k", &service_target()])
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
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

/// Enable/disable launch at login.
fn daemon_set_login(on: bool) -> Result<(), GuiError> {
    #[cfg(target_os = "linux")]
    {
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
            // Unified model: the login toggle *is* registration. On =
            // register (→ "Yerd" Login Items entry + runs at login + now);
            // off = unregister (removes the entry + stops it).
            if on {
                smapp_enable()
            } else {
                // Unregister the SMAppService agent (only when actually
                // registered — `unregister()` on a never-registered service is a
                // no-op error path) AND tear down any leftover legacy loose agent
                // so the toggle doesn't appear stuck "on" for upgrade users who
                // only ever had the pre-SMAppService loose plist.
                if smapp_registered() {
                    crate::smappservice::unregister()?;
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
        let _ = on;
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
    /// GUI-at-login (from the autostart plugin — authoritative).
    pub gui: bool,
    /// Start-the-GUI-minimized preference.
    pub gui_minimized: bool,
    /// macOS only: the daemon is registered but **waiting for the user to enable
    /// it in System Settings → Login Items** (SMAppService `requiresApproval`).
    /// Drives a first-run banner; always false elsewhere.
    pub daemon_pending_approval: bool,
}

/// Current "run daemon at login" state for the General tab. On the macOS
/// SMAppService path this is the live registration status (the toggle *is*
/// registration), plus a **read-only** reconciliation: a leftover loose agent (a
/// translocated first run, or a pre-SMAppService release) reports as on so the
/// UI doesn't lie — the next explicit enable/start runs the safe
/// `cleanup_legacy()`. Linux + the macOS fallback use the stored intent flag.
fn daemon_enabled(settings: &GuiSettings, supported: bool) -> bool {
    #[cfg(target_os = "macos")]
    if use_smappservice() {
        let legacy = plist_path().map(|p| p.exists()).unwrap_or(false);
        return smapp_registered() || legacy;
    }
    supported && settings.daemon_autostart
}

/// macOS SMAppService `requiresApproval` — registered but pending the user's
/// toggle in Login Items. Always false on the fallback path / other OSes.
fn daemon_pending_approval() -> bool {
    #[cfg(target_os = "macos")]
    if use_smappservice() {
        return crate::smappservice::status()
            .map(|s| s == crate::smappservice::STATUS_REQUIRES_APPROVAL)
            .unwrap_or(false);
    }
    false
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<AutostartState, GuiError> {
    let settings = load_settings();
    let supported = manager_available();
    let gui = app
        .autolaunch()
        .is_enabled()
        .map_err(|e| GuiError::internal(format!("could not query GUI autostart: {e}")))?;
    Ok(AutostartState {
        daemon: daemon_enabled(&settings, supported),
        daemon_supported: supported,
        gui,
        gui_minimized: settings.gui_minimized,
        daemon_pending_approval: daemon_pending_approval(),
    })
}

#[tauri::command]
pub fn set_autostart_daemon(on: bool) -> Result<(), GuiError> {
    daemon_set_login(on)?;
    let mut s = load_settings();
    s.daemon_autostart = on;
    save_settings(&s)
}

#[tauri::command]
pub fn set_autostart_gui(app: tauri::AppHandle, on: bool) -> Result<(), GuiError> {
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
