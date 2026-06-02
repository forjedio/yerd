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

/// Write the LaunchAgent plist and bootstrap it (idempotent — an already-loaded
/// agent is fine). `RunAtLoad` + `KeepAlive{SuccessfulExit:false}`: it relaunches
/// at login *when enabled* and after a crash, but a clean stop stays stopped.
#[cfg(target_os = "macos")]
fn ensure_bootstrapped() -> Result<(), GuiError> {
    let yerdd = crate::daemon::resolve_yerdd()
        .ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
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
        ensure_bootstrapped()?;
        run_ok("launchctl", &["kickstart", "-k", &service_target()])
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
        ensure_bootstrapped()?;
        run_ok(
            "launchctl",
            &[if on { "enable" } else { "disable" }, &service_target()],
        )
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
        daemon: supported && settings.daemon_autostart,
        daemon_supported: supported,
        gui,
        gui_minimized: settings.gui_minimized,
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
