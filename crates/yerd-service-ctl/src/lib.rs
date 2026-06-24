//! Start / stop / restart control for the `yerdd` daemon service.
//!
//! One place for the platform service mechanics so the GUI, the `bin/yerd`
//! self-update applier, and the uninstaller don't each re-implement them (the
//! applier `bin/yerd` cannot depend on the GUI binary — strict downhill
//! dep-flow). The logic mirrors the GUI's existing `autostart`/`daemon` modules:
//!
//! - **macOS:** `launchctl kill SIGTERM gui/$uid/dev.yerd.daemon` to stop, and
//!   `launchctl kickstart -k …` to (re)start the registered `LaunchAgent`. The
//!   `SMAppService` *registration* itself is the GUI's job (it owns the objc
//!   bindings); this crate only drives `launchctl` against the already-known
//!   label.
//! - **Linux:** `systemctl --user {stop,restart} yerd` when a systemd user
//!   instance is reachable, else SIGTERM the running pid and (for start) a
//!   detached `yerdd serve`.
//!
//! No `unsafe`, no async, no IPC, no network — it shells out to the platform
//! tools and uses `nix` safe wrappers for `kill`/`getuid`, so its dependency
//! graph stays minimal.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

/// The launchd label the daemon is registered under (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const DAEMON_LABEL: &str = "dev.yerd.daemon";
/// The systemd `--user` unit name (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SYSTEMD_UNIT: &str = "yerd";
/// The exact process name to match when falling back to signalling by pid.
const DAEMON_PROCESS: &str = "yerdd";

/// A daemon service-control failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// Could not launch the platform service tool (`launchctl`/`systemctl`) or a
    /// detached `yerdd`.
    #[error("service control failed: {0}")]
    Spawn(String),
    /// The service tool ran but reported failure.
    #[error("{tool} failed: {message}")]
    Tool {
        /// The tool that failed.
        tool: &'static str,
        /// Captured stderr / a short reason.
        message: String,
    },
    /// The platform has no supported daemon-management mechanism.
    #[error("daemon service control is not supported on this platform")]
    Unsupported,
}

/// Controls the `yerdd` daemon service. Construct with the path to the `yerdd`
/// binary (used only for the Linux no-systemd detached-spawn fallback).
#[derive(Debug, Clone)]
pub struct ServiceCtl {
    yerdd_path: PathBuf,
}

impl ServiceCtl {
    /// `yerdd_path` is the daemon binary to spawn when no service manager is
    /// available (Linux without a systemd user instance).
    #[must_use]
    pub fn new(yerdd_path: impl Into<PathBuf>) -> Self {
        Self {
            yerdd_path: yerdd_path.into(),
        }
    }

    /// Stop the daemon. Best-effort: asks the service manager to stop it, then
    /// SIGTERMs any still-running `yerdd` pid (covers `cargo run` / bare
    /// `yerdd serve` that no service manages). The daemon exits cleanly on
    /// SIGTERM.
    pub fn stop(&self) {
        service_stop();
        sigterm_running();
    }

    /// Start the daemon via the service manager, or a detached spawn when none
    /// is available.
    pub fn start(&self) -> Result<(), ServiceError> {
        service_start(&self.yerdd_path)
    }

    /// Restart the daemon so it picks up a freshly-swapped binary.
    ///
    /// macOS uses `launchctl kickstart -k` (kill-then-restart of the registered
    /// job in one step). Linux uses `systemctl --user restart` when available,
    /// else stop → wait-for-exit → start.
    pub fn restart(&self) -> Result<(), ServiceError> {
        #[cfg(target_os = "macos")]
        {
            kickstart()
        }
        #[cfg(target_os = "linux")]
        {
            if systemd_user_available() {
                return run_ok("systemctl", &["--user", "restart", SYSTEMD_UNIT]);
            }
            self.stop();
            wait_for_exit();
            self.start()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = &self.yerdd_path;
            Err(ServiceError::Unsupported)
        }
    }
}

// ── stop ─────────────────────────────────────────────────────────────────────

fn service_stop() {
    #[cfg(target_os = "macos")]
    {
        let _ = run_ok("launchctl", &["kill", "SIGTERM", &service_target()]);
    }
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            let _ = run_ok("systemctl", &["--user", "stop", SYSTEMD_UNIT]);
        }
    }
}

/// SIGTERM every running `yerdd` owned by the current user (best-effort).
fn sigterm_running() {
    #[cfg(unix)]
    for pid in running_pids() {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
}

// ── start ────────────────────────────────────────────────────────────────────

fn service_start(yerdd_path: &Path) -> Result<(), ServiceError> {
    #[cfg(target_os = "macos")]
    {
        let _ = yerdd_path;
        kickstart()
    }
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            return run_ok("systemctl", &["--user", "start", SYSTEMD_UNIT]);
        }
        spawn_detached(yerdd_path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = yerdd_path;
        Err(ServiceError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn kickstart() -> Result<(), ServiceError> {
    // `-k` kills the job (if running) then restarts it, re-exec'ing the binary
    // at the plist's path — which is what we want after a bundle swap.
    run_ok("launchctl", &["kickstart", "-k", &service_target()])
}

/// Spawn `yerdd serve` in its own process group with null stdio, so it survives
/// the caller exiting. Used only on Linux without a systemd user instance.
#[cfg(target_os = "linux")]
fn spawn_detached(yerdd_path: &Path) -> Result<(), ServiceError> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    Command::new(yerdd_path)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // New process group → detached from the caller's controlling terminal
        // and signals. Safe (no pre_exec / unsafe needed).
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| ServiceError::Spawn(format!("{}: {e}", yerdd_path.display())))
}

// ── helpers ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn service_target() -> String {
    format!("gui/{}/{DAEMON_LABEL}", current_uid())
}

#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// Running `yerdd` pids owned by the current user, via `pgrep`. Empty on any
/// failure (no `pgrep`, none running).
#[cfg(unix)]
fn running_pids() -> Vec<i32> {
    let uid = current_uid().to_string();
    let out = Command::new("pgrep")
        .args(["-x", DAEMON_PROCESS, "-U", &uid])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_pids(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Parse `pgrep` stdout (one pid per line) into pids, skipping junk.
fn parse_pids(stdout: &str) -> Vec<i32> {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect()
}

/// Block (bounded) until no `yerdd` is running, so a restart spawns onto a freed
/// binary. Caps at ~5s; the daemon exits within well under a second normally.
#[cfg(target_os = "linux")]
fn wait_for_exit() {
    use std::time::Duration;
    for _ in 0..50 {
        if running_pids().is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// True when a systemd `--user` instance is reachable (`show-environment` exits
/// 0 only against a live user manager).
#[cfg(target_os = "linux")]
fn systemd_user_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run a command, mapping a non-zero exit (or spawn failure) to [`ServiceError`].
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn run_ok(tool: &'static str, args: &[&str]) -> Result<(), ServiceError> {
    let out = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| ServiceError::Spawn(format!("{tool}: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ServiceError::Tool {
            tool,
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_pids_reads_one_per_line_and_skips_junk() {
        assert_eq!(parse_pids("123\n456\n"), vec![123, 456]);
        assert_eq!(parse_pids("  789  \n"), vec![789]);
        assert_eq!(parse_pids(""), Vec::<i32>::new());
        assert_eq!(parse_pids("not-a-pid\n42\n"), vec![42]);
    }

    #[test]
    fn service_ctl_holds_the_yerdd_path() {
        let ctl = ServiceCtl::new("/usr/lib/yerd/yerdd");
        assert_eq!(ctl.yerdd_path, PathBuf::from("/usr/lib/yerd/yerdd"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_service_target_is_gui_scoped() {
        let t = service_target();
        assert!(t.starts_with("gui/"), "{t}");
        assert!(t.ends_with("/dev.yerd.daemon"), "{t}");
    }
}
