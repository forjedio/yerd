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
    search_dirs()
        .into_iter()
        .map(|d| d.join(name))
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
    tokio::task::spawn_blocking(move || crate::autostart::daemon_start(nudge))
        .await
        .map_err(|e| GuiError::internal(format!("start task failed: {e}")))?
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
    match crate::ipc::exchange(&yerd_ipc::Request::Status).await {
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

/// Spawn `yerdd serve` detached so it survives the GUI exiting (its own
/// session, stdio to /dev/null). Used only on the no-service-manager path
/// (Linux without systemd `--user`; macOS always has launchd).
#[cfg(target_os = "linux")]
pub(crate) fn spawn_detached() -> Result<(), GuiError> {
    let yerdd = resolve_yerdd().ok_or_else(|| GuiError::internal("yerdd is not installed"))?;
    let mut cmd = std::process::Command::new(&yerdd);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
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
