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
//!   label.
//! - **Linux:** `systemctl --user {stop,restart} yerd` when a systemd user
//!   instance is reachable, else SIGTERM the running pid and (for start) a
//!   detached `yerdd serve`.
//! - **Windows:** there is no service manager in play (per-user logon autostart,
//!   not an SCM service). `stop` force-kills `yerdd.exe` with absolute-path
//!   `taskkill` (Job Objects reap the children), `start` spawns a hidden detached
//!   `yerdd serve`, and `restart` is stop -> bounded `tasklist` poll -> start.
//!   The crate also owns the login-entry mechanics that Unix keeps in the GUI:
//!   [`enable_at_login`] / [`disable_at_login`] / [`autostart_enabled`] manage
//!   the `Yerd Daemon` HKCU `Run` value and its Task-Manager override.
//!
//! No `unsafe`, no async, no IPC, no network - it shells out to the platform
//! tools and uses `nix` safe wrappers for `kill`/`getuid` (Unix) and `winreg`
//! for the user's own hive (Windows), so its dependency graph stays minimal.

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
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
    /// A Windows registry access (the HKCU `Run` autostart entry) failed.
    #[error("registry access failed: {0}")]
    Registry(String),
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
            if !wait_for_exit() {
                return Err(ServiceError::Tool {
                    tool: "yerdd",
                    message: "daemon did not exit before the restart timeout".to_owned(),
                });
            }
            self.start()
        }
        #[cfg(windows)]
        {
            self.stop();
            if !wait_for_exit() {
                return Err(ServiceError::Tool {
                    tool: "yerdd",
                    message: "daemon did not exit before the restart timeout".to_owned(),
                });
            }
            self.start()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
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
    #[cfg(windows)]
    {
        taskkill_daemon();
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
        if systemd_user_available() {
            return run_ok("systemctl", &["--user", "start", SYSTEMD_UNIT]);
        }
        spawn_detached(yerdd_path)
    }
    #[cfg(windows)]
    {
        spawn_detached_windows(yerdd_path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = yerdd_path;
        Err(ServiceError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn kickstart() -> Result<(), ServiceError> {
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
    format!("gui/{}/{DAEMON_LABEL}", current_uid())
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
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_pids(stdout: &str) -> Vec<i32> {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect()
}

/// Block (bounded) until no `yerdd` is running, so a restart spawns onto a freed
/// binary. Returns `true` once it exits, or `false` on the ~5s timeout (the
/// daemon normally exits well under a second). The caller must not start a new
/// daemon on `false` - the old one may still hold the socket/ports.
#[cfg(target_os = "linux")]
fn wait_for_exit() -> bool {
    use std::time::Duration;
    for _ in 0..50 {
        if running_pids().is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

// ── Windows: per-user logon autostart (HKCU Run) + process control ─────────────

/// The HKCU `Run` key holding per-user logon autostart entries.
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The Task-Manager startup-override key: a 12-byte `REG_BINARY` per entry that
/// records an enabled/disabled decision made in Task Manager > Startup.
#[cfg(windows)]
const STARTUP_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The `Run` value name for Yerd's daemon entry, deliberately distinct from the
/// GUI's `Yerd` value so the two login entries never collide.
#[cfg(windows)]
const RUN_VALUE_NAME: &str = "Yerd Daemon";
/// The daemon image name matched by `taskkill` / `tasklist`.
#[cfg(windows)]
const DAEMON_EXE: &str = "yerdd.exe";
/// Spawn flags for the detached hidden daemon (safe std `creation_flags`). A
/// hidden console still receives logoff/shutdown control events, so the daemon
/// can drain gracefully - `DETACHED_PROCESS` would silence those.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// See [`CREATE_NO_WINDOW`]; a fresh process group so console signals to the
/// parent do not propagate to the detached daemon.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// The 12-byte `StartupApproved` payload meaning "enabled" (auto-launch parity):
/// a `0x02` state flag followed by an all-zero disable timestamp.
#[cfg_attr(not(windows), allow(dead_code))]
const STARTUP_APPROVED_ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// The `Run` value data for the daemon entry: the quoted absolute `yerdd.exe`
/// path plus `serve --detach`. The path is always quoted because Windows profile
/// paths routinely contain spaces (`C:\Users\John Smith\...`); auto-launch omits
/// the quotes, a latent bug we deliberately do not copy.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_value_data(yerdd: &Path) -> String {
    format!("\"{}\" serve --detach", yerdd.display())
}

/// Whether a `StartupApproved` payload means "enabled". Mirrors auto-launch: an
/// absent value (`None`), or one shorter than 8 bytes, reads as enabled; a longer
/// payload is enabled only when its last 8 bytes (the disable FILETIME) are zero.
#[cfg_attr(not(windows), allow(dead_code))]
fn startup_approved_enabled(bytes: Option<&[u8]>) -> bool {
    match bytes {
        Some(b) if b.len() >= 8 => b.iter().rev().take(8).all(|&v| v == 0),
        _ => true,
    }
}

/// Whether `tasklist /FO CSV /NH` output lists `exe`. A CSV data row starts with
/// the quoted image name; the localized "no tasks" INFO line never does, so this
/// dodges the locale trap that string-matching `sc query` output would hit.
#[cfg_attr(not(windows), allow(dead_code))]
fn tasklist_lists(stdout: &str, exe: &str) -> bool {
    let needle = format!("\"{exe}\"");
    stdout.lines().any(|l| l.trim_start().starts_with(&needle))
}

/// Absolute path to a `System32` executable, from `%SystemRoot%` (falling back to
/// the conventional location), so the lookup never trusts `PATH`. Mirrors
/// `bin/yerd`'s `uninstall.rs::system32_exe`.
#[cfg(windows)]
fn system32_exe(name: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(name)
}

/// Force-kill any `yerdd.exe` for this user via absolute-path `taskkill`. An
/// unelevated `taskkill` only reaps same-user processes (the `pgrep -U` intent);
/// Phase 2 Job Objects reap the php-cgi/DB/mail children when the daemon dies -
/// the same trade `uninstall.rs::stop_daemon` already ships. Best-effort.
#[cfg(windows)]
fn taskkill_daemon() {
    let taskkill = system32_exe("taskkill.exe");
    let _ = Command::new(&taskkill)
        .args(["/F", "/IM", DAEMON_EXE])
        .output();
}

/// Spawn `yerdd serve` hidden and detached with null stdio, so it survives the
/// caller exiting and shows no console window. The Windows mirror of the Linux
/// no-systemd `spawn_detached`.
#[cfg(windows)]
fn spawn_detached_windows(yerdd_path: &Path) -> Result<(), ServiceError> {
    use std::os::windows::process::CommandExt as _;
    use std::process::Stdio;

    Command::new(yerdd_path)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| ServiceError::Spawn(format!("{}: {e}", yerdd_path.display())))
}

/// Whether any `yerdd.exe` is running, via absolute-path `tasklist` CSV output.
/// Empty/failed output reads as "not running".
#[cfg(windows)]
fn yerdd_running() -> bool {
    let tasklist = system32_exe("tasklist.exe");
    let filter = format!("IMAGENAME eq {DAEMON_EXE}");
    match Command::new(&tasklist)
        .args(["/FO", "CSV", "/NH", "/FI", &filter])
        .output()
    {
        Ok(o) => tasklist_lists(&String::from_utf8_lossy(&o.stdout), DAEMON_EXE),
        Err(_) => false,
    }
}

/// Block (bounded) until no `yerdd.exe` is running, so a restart spawns onto a
/// freed pipe/ports. `true` once it exits, `false` on the ~5s timeout. The caller
/// must not start a new daemon on `false` - the old one may still hold the pipe.
/// The Windows mirror of the Linux `wait_for_exit`.
#[cfg(windows)]
fn wait_for_exit() -> bool {
    use std::time::Duration;
    for _ in 0..50 {
        if !yerdd_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Register the daemon to start at this user's logon.
///
/// Writes the HKCU `Run` value `Yerd Daemon` = `"<yerdd.exe>" serve --detach`,
/// then, if a Task-Manager override already exists for the entry, repairs it to
/// "enabled" so re-enabling from Yerd beats a prior Task-Manager "Disable"
/// (exactly what auto-launch does for the GUI entry). Idempotent and
/// unprivileged (the invoking user's own hive).
///
/// # Errors
/// [`ServiceError::Registry`] if the `Run` key can't be opened or written.
#[cfg(windows)]
pub fn enable_at_login(yerdd: &Path) -> Result<(), ServiceError> {
    write_run_value(RUN_KEY, STARTUP_APPROVED_KEY, RUN_VALUE_NAME, yerdd)
}

/// Remove the daemon's HKCU `Run` value. An already-absent value is `Ok` (the
/// idempotent disable/uninstall case).
///
/// # Errors
/// [`ServiceError::Registry`] if the `Run` key can't be opened, or the delete
/// fails for a reason other than the value already being absent.
#[cfg(windows)]
pub fn disable_at_login() -> Result<(), ServiceError> {
    delete_run_value(RUN_KEY, RUN_VALUE_NAME)
}

/// Whether the daemon is registered to start at logon: the `Run` value is present
/// and not overridden to "disabled" in Task Manager. Any registry error reads as
/// `false` (treated as not registered).
#[cfg(windows)]
#[must_use]
pub fn autostart_enabled() -> bool {
    read_autostart_enabled(RUN_KEY, STARTUP_APPROVED_KEY, RUN_VALUE_NAME)
}

/// Set the `Run` value and (if present) repair the `StartupApproved` override.
/// Parameterized by key/value names so tests can exercise it against a scratch
/// subkey instead of the real `Run` key.
#[cfg(windows)]
fn write_run_value(
    run_key: &str,
    approved_key: &str,
    value_name: &str,
    yerdd: &Path,
) -> Result<(), ServiceError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY};
    use winreg::{RegKey, RegValue};

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(run_key, KEY_SET_VALUE)
        .and_then(|run| run.set_value(value_name, &run_value_data(yerdd)))
        .map_err(|e| ServiceError::Registry(format!("{run_key}: {e}")))?;

    if let Ok(approved) = hkcu.open_subkey_with_flags(approved_key, KEY_READ | KEY_SET_VALUE) {
        if approved.get_raw_value(value_name).is_ok() {
            approved
                .set_raw_value(
                    value_name,
                    &RegValue {
                        vtype: REG_BINARY,
                        bytes: STARTUP_APPROVED_ENABLED.to_vec(),
                    },
                )
                .map_err(|e| ServiceError::Registry(format!("{approved_key}: {e}")))?;
        }
    }
    Ok(())
}

/// Delete `value_name` under `run_key`, tolerating an already-absent value.
/// Parameterized for the same scratch-key testing reason as [`write_run_value`].
#[cfg(windows)]
fn delete_run_value(run_key: &str, value_name: &str) -> Result<(), ServiceError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let run = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(run_key, KEY_SET_VALUE)
        .map_err(|e| ServiceError::Registry(format!("{run_key}: {e}")))?;
    match run.delete_value(value_name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ServiceError::Registry(format!("{value_name}: {e}"))),
    }
}

/// Whether `value_name` under `run_key` is present and not disabled by its
/// `StartupApproved` override. Parameterized for scratch-key testing.
#[cfg(windows)]
fn read_autostart_enabled(run_key: &str, approved_key: &str, value_name: &str) -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let present = hkcu
        .open_subkey_with_flags(run_key, KEY_READ)
        .and_then(|run| run.get_value::<String, _>(value_name))
        .is_ok();
    if !present {
        return false;
    }
    let override_bytes = hkcu
        .open_subkey_with_flags(approved_key, KEY_READ)
        .ok()
        .and_then(|approved| approved.get_raw_value(value_name).ok())
        .map(|v| v.bytes);
    startup_approved_enabled(override_bytes.as_deref())
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

    #[test]
    fn run_value_data_quotes_the_path_and_appends_serve_detach() {
        assert_eq!(
            run_value_data(Path::new(r"C:\Program Files\yerd\yerdd.exe")),
            "\"C:\\Program Files\\yerd\\yerdd.exe\" serve --detach"
        );
        assert_eq!(
            run_value_data(Path::new(r"C:\Users\John Smith\bin\yerdd.exe")),
            "\"C:\\Users\\John Smith\\bin\\yerdd.exe\" serve --detach"
        );
    }

    #[test]
    fn startup_approved_enabled_matches_auto_launch_semantics() {
        assert!(startup_approved_enabled(None), "absent value is enabled");
        assert!(
            startup_approved_enabled(Some(&STARTUP_APPROVED_ENABLED)),
            "the canonical enabled payload is enabled"
        );
        assert!(
            !startup_approved_enabled(Some(&[
                0x03, 0, 0, 0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88
            ])),
            "a non-zero disable timestamp is disabled"
        );
        assert!(
            startup_approved_enabled(Some(&[0x02, 0, 0])),
            "a short payload (< 8 bytes) reads as enabled, like auto-launch"
        );
        assert!(
            startup_approved_enabled(Some(&[0, 0, 0, 0, 0, 0, 0, 0])),
            "exactly 8 zero bytes is enabled"
        );
    }

    #[test]
    fn tasklist_lists_detects_a_csv_row_only() {
        let csv = "\"yerdd.exe\",\"1234\",\"Console\",\"1\",\"12,345 K\"\r\n";
        assert!(tasklist_lists(csv, "yerdd.exe"));
        assert!(
            !tasklist_lists(
                "INFO: No tasks are running which match the specified criteria.\r\n",
                "yerdd.exe"
            ),
            "the localized no-tasks line must not count as a match"
        );
        assert!(!tasklist_lists("", "yerdd.exe"));
        assert!(
            !tasklist_lists("\"yerd.exe\",\"9\",\"Console\"\r\n", "yerdd.exe"),
            "a different image name must not match"
        );
    }

    /// End-to-end registry round-trip against a throwaway `HKCU\Software\YerdTest-<pid>`
    /// subtree, never the real `Run` key. Exercises enable -> read -> a
    /// Task-Manager disable override -> re-enable repair -> idempotent disable.
    #[cfg(windows)]
    #[test]
    fn enable_read_disable_round_trip_on_scratch_key() {
        use winreg::enums::{HKEY_CURRENT_USER, REG_BINARY};
        use winreg::{RegKey, RegValue};

        /// Deletes the scratch subtree on drop, even if an assertion unwinds.
        struct Guard(String);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
            }
        }

        let base = format!(r"Software\YerdTest-{}", std::process::id());
        let run_key = format!(r"{base}\Run");
        let approved_key = format!(r"{base}\StartupApproved");
        let name = "Yerd Daemon";
        let _guard = Guard(base.clone());

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.create_subkey(&run_key).unwrap();
        let (approved, _) = hkcu.create_subkey(&approved_key).unwrap();

        let exe = PathBuf::from(r"C:\Users\John Smith\yerd\yerdd.exe");

        assert!(!read_autostart_enabled(&run_key, &approved_key, name));

        write_run_value(&run_key, &approved_key, name, &exe).unwrap();
        let (run, _) = hkcu.create_subkey(&run_key).unwrap();
        let data: String = run.get_value(name).unwrap();
        assert_eq!(data, run_value_data(&exe));
        assert!(read_autostart_enabled(&run_key, &approved_key, name));

        approved
            .set_raw_value(
                name,
                &RegValue {
                    vtype: REG_BINARY,
                    bytes: vec![0x03, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8],
                },
            )
            .unwrap();
        assert!(
            !read_autostart_enabled(&run_key, &approved_key, name),
            "a Task-Manager disable override wins"
        );

        write_run_value(&run_key, &approved_key, name, &exe).unwrap();
        assert!(
            read_autostart_enabled(&run_key, &approved_key, name),
            "re-enabling must repair the override"
        );

        delete_run_value(&run_key, name).unwrap();
        assert!(!read_autostart_enabled(&run_key, &approved_key, name));
        delete_run_value(&run_key, name).unwrap();
    }

    /// The public probe against the real hive must never panic and returns a bool.
    #[cfg(windows)]
    #[test]
    fn autostart_enabled_smoke_is_infallible() {
        let _: bool = autostart_enabled();
    }
}
