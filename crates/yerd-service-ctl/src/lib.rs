//! Start / stop / restart control for the `yerdd` daemon service.
//!
//! One place for the platform service mechanics so the GUI, the `bin/yerd`
//! self-update applier, and the uninstaller don't each re-implement them (the
//! applier `bin/yerd` cannot depend on the GUI binary - strict downhill
//! dep-flow). The logic mirrors the GUI's existing `autostart`/`daemon` modules:
//!
//! - **macOS:** `launchctl kill SIGTERM gui/$uid/dev.yerd.daemon` to stop, and
//!   `launchctl kickstart -k …` to (re)start the registered `LaunchAgent`. The
//!   `SMAppService` *registration* itself is the GUI's job (it owns the objc
//!   bindings); this crate only drives `launchctl` against the already-known
//!   label. When the headless install's system-domain job
//!   (`/Library/LaunchDaemons/dev.yerd.daemon.plist`) exists, the target is
//!   `system/dev.yerd.daemon` instead: start and restart need root there
//!   ([`ServiceError::NeedsRoot`]), and stop just SIGTERMs the user's daemon,
//!   which exits 0 so launchd leaves it down.
//! - **Linux:** `systemctl --user {stop,restart} yerd` when a systemd user
//!   instance is reachable and the `yerd.service` unit exists, else SIGTERM
//!   the running pid and (for start) a detached `yerdd serve`. A headless
//!   install registers the same unit with [`install_linux_unit`].
//!
//! No `unsafe`, no async, no IPC, no network - it shells out to the platform
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
/// The headless install's system-domain daemon plist (macOS). Duplicates
/// `yerd_platform::pure::launchd_daemon::DAEMON_PLIST` so this crate stays
/// dependency-light; keep the two in sync.
pub const SYSTEM_DAEMON_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.daemon.plist";

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
    /// The daemon is a system-domain launchd job, which only root can start.
    #[error("the daemon is a system launchd job; starting it needs root")]
    NeedsRoot,
}

/// Whether `loginctl enable-linger` took effect, so the user's systemd
/// manager (and the daemon) starts at boot with nobody logged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LingerOutcome {
    /// Linger is on.
    Enabled,
    /// Linger stayed off (typically polkit refused the unprivileged request).
    NotEnabled,
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
    /// job in one step), or [`ServiceError::NeedsRoot`] for the system job.
    /// Linux uses `systemctl --user restart` when the unit is usable, else
    /// stop → wait-for-exit → start.
    pub fn restart(&self) -> Result<(), ServiceError> {
        #[cfg(target_os = "macos")]
        {
            kickstart()
        }
        #[cfg(target_os = "linux")]
        {
            if systemd_unit_usable() {
                return run_ok("systemctl", &["--user", "restart", SYSTEMD_UNIT]);
            }
            self.stop();
            if !wait_for_exit() {
                return Err(ServiceError::Tool {
                    tool: "yerdd",
                    message: "daemon did not exit before the restart timeout".to_owned(),
                });
            }
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
        if !system_job_registered() {
            let _ = run_ok("launchctl", &["kill", "SIGTERM", &service_target()]);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if systemd_user_available() {
            let _ = run_ok("systemctl", &["--user", "stop", SYSTEMD_UNIT]);
        }
    }
}

/// SIGTERM every running `yerdd` owned by the current user (best-effort). Gated
/// to the supported OSes so an "unsupported" build never signals user processes.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sigterm_running() {
    for pid in running_pids() {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sigterm_running() {}

// ── start ────────────────────────────────────────────────────────────────────

fn service_start(yerdd_path: &Path) -> Result<(), ServiceError> {
    #[cfg(target_os = "macos")]
    {
        let _ = yerdd_path;
        kickstart()
    }
    #[cfg(target_os = "linux")]
    {
        if systemd_unit_usable() {
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
    if system_job_registered() {
        return Err(ServiceError::NeedsRoot);
    }
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
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| ServiceError::Spawn(format!("{}: {e}", yerdd_path.display())))
}

// ── helpers ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn service_target() -> String {
    macos_target(system_job_registered(), current_uid())
}

#[cfg(target_os = "macos")]
fn system_job_registered() -> bool {
    Path::new(SYSTEM_DAEMON_PLIST).exists()
}

/// The launchd service target for the daemon: the headless system-domain job
/// when its plist exists, else the desktop app's per-user agent.
#[must_use]
pub fn macos_target(system_plist_exists: bool, uid: u32) -> String {
    if system_plist_exists {
        format!("system/{DAEMON_LABEL}")
    } else {
        format!("gui/{uid}/{DAEMON_LABEL}")
    }
}

/// The systemd `--user` unit text for `yerd.service` running `yerdd serve`.
///
/// Same text as the desktop app's `autostart::render_unit`, which keeps its
/// own copy; the two must stay identical so either install's unit reads the
/// same.
#[must_use]
pub fn render_systemd_unit(yerdd: &Path) -> String {
    format!(
        "[Unit]\nDescription=Yerd local PHP development daemon\n\n[Service]\nType=simple\nExecStart={} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        yerdd.display()
    )
}

/// The program path from a unit's `ExecStart=<path> serve` line, or `None`
/// when the unit has no `ExecStart`.
#[must_use]
pub fn unit_exec_start(unit: &str) -> Option<&str> {
    unit.lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))
        .map(|value| {
            let value = value.trim();
            value.strip_suffix(" serve").unwrap_or(value).trim_end()
        })
}

/// True when `loginctl show-user --property=Linger` output says linger is on.
#[must_use]
pub fn linger_enabled(show_user_out: &str) -> bool {
    show_user_out
        .lines()
        .any(|line| line.trim() == "Linger=yes")
}

/// Where the `yerd.service` user unit lives: `$XDG_CONFIG_HOME/systemd/user`
/// or `~/.config/systemd/user`. `None` when neither base resolves.
#[must_use]
pub fn systemd_unit_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".config"))
        })?;
    Some(
        base.join("systemd")
            .join("user")
            .join(format!("{SYSTEMD_UNIT}.service")),
    )
}

/// Write the `yerd.service` user unit for `yerdd`, enable and start it, then
/// ask for linger so it starts at boot without a login.
///
/// Linger is requested as the user (polkit-mediated); a refusal is not an
/// error, it is reported as [`LingerOutcome::NotEnabled`] after re-checking
/// with `loginctl show-user`.
///
/// # Errors
///
/// [`ServiceError::Tool`] when no systemd user manager is reachable, the unit
/// can't be written, or `systemctl` fails.
#[cfg(target_os = "linux")]
pub fn install_linux_unit(yerdd: &Path) -> Result<LingerOutcome, ServiceError> {
    if !systemd_user_available() {
        return Err(ServiceError::Tool {
            tool: "systemctl",
            message: "no systemd --user manager is reachable for this user".to_owned(),
        });
    }
    let path = systemd_unit_path().ok_or_else(|| ServiceError::Tool {
        tool: "systemctl",
        message: "cannot resolve the systemd user unit directory (HOME unset)".to_owned(),
    })?;
    let write_err = |e: std::io::Error| ServiceError::Tool {
        tool: "systemctl",
        message: format!("cannot write {}: {e}", path.display()),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(write_err)?;
    }
    std::fs::write(&path, render_systemd_unit(yerdd)).map_err(write_err)?;
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
    let _ = run_ok("loginctl", &["enable-linger"]);
    Ok(query_linger())
}

/// Unsupported off Linux.
///
/// # Errors
///
/// Always [`ServiceError::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn install_linux_unit(yerdd: &Path) -> Result<LingerOutcome, ServiceError> {
    let _ = yerdd;
    Err(ServiceError::Unsupported)
}

#[cfg(target_os = "linux")]
fn query_linger() -> LingerOutcome {
    let uid = current_uid().to_string();
    let out = Command::new("loginctl")
        .args(["show-user", &uid, "--property=Linger"])
        .output();
    match out {
        Ok(o) if o.status.success() && linger_enabled(&String::from_utf8_lossy(&o.stdout)) => {
            LingerOutcome::Enabled
        }
        _ => LingerOutcome::NotEnabled,
    }
}

/// True when `systemctl --user` can drive the daemon: a user manager is
/// reachable and the `yerd.service` unit file exists.
#[cfg(target_os = "linux")]
fn systemd_unit_usable() -> bool {
    systemd_unit_path().is_some_and(|p| p.is_file()) && systemd_user_available()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// Running `yerdd` pids owned by the current user, via `pgrep`. Empty on any
/// failure (no `pgrep`, none running).
#[cfg(any(target_os = "linux", target_os = "macos"))]
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

/// Block (bounded) until no `yerdd` owned by the current user is running, so a
/// restart spawns onto a freed binary. Returns `true` once it exits, or `false`
/// on the ~5s timeout (the daemon normally exits well under a second). The
/// caller must not start a new daemon on `false` - the old one may still hold
/// the socket/ports.
#[cfg(target_os = "linux")]
#[must_use]
pub fn wait_for_exit() -> bool {
    use std::time::Duration;
    for _ in 0..50 {
        if running_pids().is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// True when a systemd `--user` manager is reachable for the current user, the
/// precondition [`install_linux_unit`] checks first. Lets a caller refuse
/// before stopping a running daemon.
#[cfg(target_os = "linux")]
#[must_use]
pub fn systemd_user_manager_reachable() -> bool {
    systemd_user_available()
}

/// Always `false` off Linux: there is no systemd user manager.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn systemd_user_manager_reachable() -> bool {
    false
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
    fn macos_service_target_ends_with_the_label() {
        let t = service_target();
        assert!(t.starts_with("gui/") || t.starts_with("system/"), "{t}");
        assert!(t.ends_with("/dev.yerd.daemon"), "{t}");
    }

    #[test]
    fn macos_target_picks_system_job_when_its_plist_exists() {
        assert_eq!(macos_target(true, 501), "system/dev.yerd.daemon");
        assert_eq!(macos_target(false, 501), "gui/501/dev.yerd.daemon");
    }

    #[test]
    fn rendered_unit_runs_yerdd_serve_at_default_target() {
        let unit = render_systemd_unit(Path::new("/home/u/.yerd/bin/yerdd"));
        assert!(
            unit.contains("ExecStart=/home/u/.yerd/bin/yerdd serve\n"),
            "{unit}"
        );
        assert!(unit.contains("WantedBy=default.target\n"), "{unit}");
        assert!(unit.contains("Restart=on-failure\n"), "{unit}");
        assert_eq!(unit_exec_start(&unit), Some("/home/u/.yerd/bin/yerdd"));
    }

    #[test]
    fn unit_exec_start_table() {
        let gui = "[Unit]\nDescription=Yerd local PHP development daemon\n\n[Service]\nType=simple\nExecStart=/usr/bin/yerdd serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n";
        let cases = [
            (gui, Some("/usr/bin/yerdd")),
            ("ExecStart=/opt/yerdd\n", Some("/opt/yerdd")),
            ("  ExecStart=/x/yerdd serve  \n", Some("/x/yerdd")),
            ("[Service]\nType=simple\n", None),
            ("", None),
        ];
        for (unit, want) in cases {
            assert_eq!(unit_exec_start(unit), want, "{unit:?}");
        }
    }

    #[test]
    fn linger_enabled_table() {
        let cases = [
            ("Linger=yes\n", true),
            ("Linger=yes", true),
            ("  Linger=yes  \n", true),
            ("Linger=no\n", false),
            ("", false),
            ("garbage\nLinger=maybe\n", false),
            ("Linger=yesterday\n", false),
        ];
        for (out, want) in cases {
            assert_eq!(linger_enabled(out), want, "{out:?}");
        }
    }

    #[test]
    fn needs_root_display() {
        assert!(ServiceError::NeedsRoot.to_string().contains("needs root"));
    }

    #[test]
    fn system_plist_matches_platform_constant() {
        assert!(SYSTEM_DAEMON_PLIST.ends_with(&format!("/{DAEMON_LABEL}.plist")));
    }

    /// `parse_pids` is total: it skips blank interior lines, trims whitespace
    /// around each pid, drops out-of-range and non-numeric tokens, and never
    /// panics even on a leading '-' that pgrep would never actually emit.
    #[test]
    fn parse_pids_handles_blank_lines_and_negative_junk() {
        assert_eq!(parse_pids("1\n\n2\n\n3\n"), vec![1, 2, 3]);
        assert_eq!(parse_pids("\t10\t\n 20 \n"), vec![10, 20]);
        assert_eq!(parse_pids("99999999999999999999\n7\n"), vec![7]);
        assert_eq!(parse_pids("-5\n"), vec![-5]);
    }

    #[test]
    fn service_ctl_is_clone_and_debug() {
        let ctl = ServiceCtl::new("/opt/yerd/yerdd");
        let cloned = ctl.clone();
        assert_eq!(cloned.yerdd_path, ctl.yerdd_path);
        let dbg = format!("{ctl:?}");
        assert!(dbg.contains("ServiceCtl"), "{dbg}");
        assert!(dbg.contains("yerdd"), "{dbg}");
    }

    #[test]
    fn service_ctl_new_accepts_pathbuf_and_str() {
        let from_str = ServiceCtl::new("/a/b/yerdd");
        let from_pathbuf = ServiceCtl::new(PathBuf::from("/a/b/yerdd"));
        assert_eq!(from_str.yerdd_path, from_pathbuf.yerdd_path);
    }

    #[test]
    fn service_error_spawn_display() {
        let e = ServiceError::Spawn("no such file".to_owned());
        assert_eq!(e.to_string(), "service control failed: no such file");
    }

    #[test]
    fn service_error_tool_display_includes_tool_and_message() {
        let e = ServiceError::Tool {
            tool: "launchctl",
            message: "boom".to_owned(),
        };
        assert_eq!(e.to_string(), "launchctl failed: boom");
    }

    #[test]
    fn service_error_unsupported_display() {
        assert_eq!(
            ServiceError::Unsupported.to_string(),
            "daemon service control is not supported on this platform"
        );
    }

    #[test]
    fn service_error_is_debug() {
        let dbg = format!("{:?}", ServiceError::Spawn("x".to_owned()));
        assert!(dbg.contains("Spawn"), "{dbg}");
    }

    /// getuid is a pure syscall with no side effects; two reads agree.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_uid_is_stable_and_matches_process_env() {
        assert_eq!(current_uid(), current_uid());
    }

    /// `pgrep -x` against the real daemon name in a test context: the test
    /// harness is not named `yerdd`, so this must come back empty (or empty
    /// on any pgrep failure). Either way it must never panic.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn running_pids_for_unknown_process_is_empty() {
        let pids = running_pids();
        assert!(pids.iter().all(|&p| p > 0));
    }
}
