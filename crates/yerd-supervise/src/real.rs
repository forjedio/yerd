//! Production impls of [`crate::traits::Clock`] and
//! [`crate::traits::ProcessSpawner`].

use std::io;
use std::process::Command as StdCommand;
use std::time::Instant;

use async_trait::async_trait;

use crate::error::ExitReason;
use crate::supervisor::{KillSignal, StopProtocol};
use crate::traits::{ChildHandle, Clock, ProcessSpawner};

/// `std::time::Instant::now()` wrapper.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Best-effort SIGKILL to the entire process group led by `leader_pid`.
///
/// A spawned leader's `kill_on_drop(true)` only SIGKILLs the **direct** child,
/// so any grandchild it forked (e.g. the bootstrap server a `mariadb-install-db`
/// script launches) survives. When a task owning such a leader is dropped before
/// it can `wait()` - a daemon shutting down mid-init - call this to reap the
/// whole subtree. Requires the leader to have been spawned into its own process
/// group (`process_group(0)`), so its PID doubles as the group id. No-op off
/// Unix (Windows worker teardown is a Phase 2 job-object ticket, as for `kill`).
///
/// A `leader_pid` of 0 is ignored: `killpg(0, ..)` targets the *caller's* own
/// process group, so it would signal the daemon itself. A real spawned child PID
/// is never 0; this only guards a future or mistaken caller.
#[cfg(unix)]
pub fn kill_process_group(leader_pid: u32) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    if let Some(pid) = group_signal_target(leader_pid) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

/// The `i32` PID to hand `killpg`, or `None` when the group must not be signalled:
/// a `leader_pid` of 0 (`killpg(0, ..)` hits the caller's own group) or one that
/// overflows `i32`. Pure, so the 0-guard is tested without issuing a real signal.
#[cfg(unix)]
fn group_signal_target(leader_pid: u32) -> Option<i32> {
    if leader_pid == 0 {
        return None;
    }
    i32::try_from(leader_pid).ok()
}

/// Non-Unix stub: process-group reaping is a Phase 2 job-object ticket, so this
/// is a no-op (see the Unix impl for the semantics).
#[cfg(not(unix))]
pub fn kill_process_group(_leader_pid: u32) {}

/// Spawns commands via `tokio::process::Command`, sets `kill_on_drop(true)` so
/// unexpected crashes of the daemon take the child down with them.
pub struct TokioProcessSpawner;

impl ProcessSpawner for TokioProcessSpawner {
    type Child = TokioChild;

    fn spawn(&self, cmd: StdCommand) -> Result<TokioChild, io::Error> {
        let mut tokio_cmd = tokio::process::Command::from(cmd);
        tokio_cmd.kill_on_drop(true);
        let child = spawn_retrying_text_file_busy(&mut tokio_cmd)?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("child has no pid"))?;
        Ok(TokioChild { inner: child, pid })
    }
}

/// Spawn `cmd`, retrying on `ETXTBSY` ("text file busy").
///
/// A multithreaded program that writes an executable and then execs it can hit
/// `ETXTBSY` transiently: the kernel refuses to exec a file while any fd still
/// holds it open for writing, and a sibling thread's not-yet-closed writer fd
/// (or one snapshotted into a concurrent `fork`) can briefly hold it. Because
/// Rust opens files `O_CLOEXEC`, that inherited fd is dropped the instant the
/// racing child execs, so the window is very short. This is a synchronous trait
/// method that may run on a Tokio worker, so it must not block the worker with a
/// timed sleep; instead each retry `yield_now()`s (a cooperative hand-off to the
/// runnable fd-closing thread) before trying again. The first attempt succeeds
/// in the overwhelmingly common case, so the happy path pays nothing.
fn spawn_retrying_text_file_busy(
    cmd: &mut tokio::process::Command,
) -> io::Result<tokio::process::Child> {
    const MAX_ATTEMPTS: usize = 20;
    let mut result = cmd.spawn();
    let mut attempts = 1;
    while attempts < MAX_ATTEMPTS && matches!(&result, Err(e) if is_text_file_busy(e)) {
        std::thread::yield_now();
        result = cmd.spawn();
        attempts += 1;
    }
    result
}

/// Whether `e` is `ETXTBSY` (executable busy). Matched on the raw errno rather
/// than `io::ErrorKind::ExecutableFileBusy` to stay within the crate's 1.77 MSRV
/// (that variant stabilised in 1.83).
#[cfg(unix)]
fn is_text_file_busy(e: &io::Error) -> bool {
    e.raw_os_error() == Some(nix::libc::ETXTBSY)
}

#[cfg(not(unix))]
fn is_text_file_busy(_e: &io::Error) -> bool {
    false
}

/// Production [`ChildHandle`] wrapping `tokio::process::Child`.
pub struct TokioChild {
    inner: tokio::process::Child,
    pid: u32,
}

#[async_trait]
impl ChildHandle for TokioChild {
    fn id(&self) -> u32 {
        self.pid
    }

    fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error> {
        Ok(self.inner.try_wait()?.map(ExitReason::from_status))
    }

    async fn wait(&mut self) -> Result<ExitReason, io::Error> {
        Ok(ExitReason::from_status(self.inner.wait().await?))
    }

    async fn kill(&mut self, signal: KillSignal, protocol: StopProtocol) -> Result<(), io::Error> {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, killpg, Signal};
            use nix::unistd::Pid;
            let pid_i32 =
                i32::try_from(self.pid).map_err(|_| io::Error::other("pid overflows i32"))?;
            let pid = Pid::from_raw(pid_i32);
            let result = match (signal, protocol) {
                (KillSignal::Kill, _) => killpg(pid, Signal::SIGKILL),
                (KillSignal::Term, StopProtocol::GroupTerm) => killpg(pid, Signal::SIGTERM),
                (KillSignal::Term, StopProtocol::MasterInterrupt) => kill(pid, Signal::SIGINT),
            };
            result.map_err(|e| io::Error::other(e.to_string()))
        }
        #[cfg(windows)]
        {
            // TODO(Phase 2): worker leak on Windows - needs job-object teardown via yerd-helper.
            let _ = (signal, protocol);
            self.inner.kill().await
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{group_signal_target, is_text_file_busy};
    use std::io;

    #[test]
    fn group_signal_target_rejects_zero_and_overflow() {
        assert_eq!(group_signal_target(0), None);
        assert_eq!(group_signal_target(1234), Some(1234));
        assert_eq!(
            group_signal_target(u32::MAX),
            None,
            "a PID that overflows i32 must not be signalled"
        );
    }

    #[test]
    fn is_text_file_busy_matches_only_etxtbsy() {
        assert!(is_text_file_busy(&io::Error::from_raw_os_error(
            nix::libc::ETXTBSY
        )));
        assert!(!is_text_file_busy(&io::Error::from_raw_os_error(
            nix::libc::ENOENT
        )));
        assert!(!is_text_file_busy(&io::Error::other("not an os error")));
    }
}
