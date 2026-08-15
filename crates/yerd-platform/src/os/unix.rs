//! Process and executable helpers shared by the Linux and macOS adapters.
//!
//! Both adapters need the same two primitives: find an executable file in a
//! list of directories, and start a long-lived GUI process while still
//! reporting an immediate failure. Keeping one copy here stops the two edges
//! drifting apart.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a freshly spawned launcher is watched before it is treated as
/// running in the background. Long enough to catch "exec succeeded but the
/// program immediately rejected its arguments", short enough not to stall a
/// click.
pub(crate) const DEFAULT_STARTUP_WINDOW: Duration = Duration::from_millis(500);

/// Poll interval inside the startup window.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// First executable file named `name` in `directories`, or `None`.
pub(crate) fn executable_in_directories<I>(name: &str, directories: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

/// Spawn `command` and report failures observed within `startup_window`.
///
/// A process still alive at the end of the window counts as launched: an IDE
/// runs for as long as the user keeps it open, so waiting for it would pin the
/// calling thread. The child is reaped on a detached thread instead. Standard
/// streams are nulled so the launched program can never keep a pipe (and with
/// it the caller) alive.
pub(crate) fn spawn_and_check(
    command: &mut Command,
    program: &str,
    startup_window: Duration,
) -> std::io::Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + startup_window;
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(std::io::Error::other(format!(
                    "{program} exited with {status}"
                )));
            }
            None if Instant::now() >= deadline => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            None => std::thread::sleep(POLL_INTERVAL),
        }
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
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn immediate_success_and_failure_are_reported() {
        assert!(spawn_and_check(&mut shell("exit 0"), "/bin/sh", DEFAULT_STARTUP_WINDOW).is_ok());
        assert!(spawn_and_check(&mut shell("exit 7"), "/bin/sh", DEFAULT_STARTUP_WINDOW).is_err());
    }

    /// A generous window means the sleeping child can never outlive the
    /// deadline and be mistaken for a successfully backgrounded launcher.
    #[test]
    fn failure_after_the_initial_poll_is_reported() {
        assert!(spawn_and_check(
            &mut shell("sleep 0.2; exit 7"),
            "/bin/sh",
            Duration::from_secs(30)
        )
        .is_err());
    }

    #[test]
    fn a_long_running_child_returns_before_it_exits() {
        let started = Instant::now();
        assert!(
            spawn_and_check(&mut shell("sleep 5"), "/bin/sh", Duration::from_millis(100)).is_ok()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn executable_lookup_accepts_only_executable_files() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("zeditor");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            executable_in_directories("zeditor", vec![directory.path().to_path_buf()]),
            Some(executable)
        );

        let plain = directory.path().join("phpstorm");
        fs::write(&plain, b"not executable\n").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            executable_in_directories("phpstorm", vec![directory.path().to_path_buf()]),
            None
        );
    }
}
