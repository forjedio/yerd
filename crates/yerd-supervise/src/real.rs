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
/// Unix, where the Job Object handles teardown (see below).
///
/// A `leader_pid` of 0 is ignored: `killpg(0, ..)` targets the *caller's* own
/// process group, so it would signal the daemon itself. A real spawned child PID
/// is never 0; this only guards a future or mistaken caller.
///
/// On Windows the Job Object supersedes this: dropping the [`TokioChild`] (and
/// hence its job) reaps the whole tree, so the non-Unix stub is a deliberate
/// no-op.
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

/// Non-Unix stub: on Windows the per-child Job Object reaps the tree when the
/// [`TokioChild`] is dropped/killed, so explicit process-group reaping is
/// unnecessary and this is a no-op (see the Unix impl for the semantics).
#[cfg(not(unix))]
pub fn kill_process_group(_leader_pid: u32) {}

/// `CREATE_NO_WINDOW` process-creation flag: a console child runs with no
/// console window at all.
///
/// Declared here rather than imported from `yerd-platform` so this crate keeps
/// its no-`yerd-*`-dependencies shape (the same deliberate exception
/// `yerd-service-ctl` makes). Safe std `creation_flags`, no FFI.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawns commands via `tokio::process::Command`, sets `kill_on_drop(true)` so
/// unexpected crashes of the daemon take the direct child down with them, and on
/// Windows assigns each child to a kill-on-close Job Object so the **whole tree**
/// (workers, init-tool grandchildren) dies with the child - the Windows
/// equivalent of the Unix process-group reaping.
///
/// Windows children additionally get `CREATE_NO_WINDOW`. Every supervised
/// program (php-cgi, mysqld, postgres, redis, meilisearch, the init tools,
/// cloudflared) is console-subsystem and `yerdd` has no console of its own, so
/// an unflagged spawn makes Windows allocate a fresh console *window* per child.
/// Applying it in the single spawn seam means no call site has to remember it.
pub struct TokioProcessSpawner;

impl ProcessSpawner for TokioProcessSpawner {
    type Child = TokioChild;

    fn spawn(&self, cmd: StdCommand) -> Result<TokioChild, io::Error> {
        let mut tokio_cmd = tokio::process::Command::from(cmd);
        tokio_cmd.kill_on_drop(true);
        #[cfg(windows)]
        tokio_cmd.creation_flags(CREATE_NO_WINDOW);
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut child = spawn_retrying_text_file_busy(&mut tokio_cmd)?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("child has no pid"))?;
        #[cfg(windows)]
        let job = match assign_to_job(&child) {
            Ok(job) => Some(job),
            Err(e) => {
                let _ = child.start_kill();
                return Err(e);
            }
        };
        Ok(TokioChild {
            inner: child,
            pid,
            #[cfg(windows)]
            job,
        })
    }
}

/// Assign `child` to a fresh Job Object set to terminate its whole process tree
/// when the job handle is closed (an explicit drop, or a `yerdd` crash). The
/// child's raw handle comes from the safe `tokio::process::Child::raw_handle()`;
/// a missing handle means the child already exited or was reaped, treated as a
/// job-setup failure so a child we cannot contain never runs (fail closed).
///
/// Residual risk: a child could `CreateProcess` a grandchild in the microseconds
/// between spawn and `assign_process`, and that grandchild would escape the job.
/// None of yerd's workloads (php-cgi, mysqld, postgres, init tools, cloudflared)
/// spawn children before finishing their own image/DLL load, so this window is
/// accepted. Closing it fully would need `CREATE_SUSPENDED` + resume, which is
/// raw `unsafe` FFI not exposed by std/tokio.
#[cfg(windows)]
fn assign_to_job(child: &tokio::process::Child) -> Result<win32job::Job, io::Error> {
    let handle = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("child has no raw handle for job assignment"))?;
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&info)
        .map_err(|e| io::Error::other(format!("create job object: {e}")))?;
    job.assign_process(handle as isize)
        .map_err(|e| io::Error::other(format!("assign process to job object: {e}")))?;
    Ok(job)
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
///
/// On Windows it also owns the [`win32job::Job`] the child is assigned to;
/// dropping the job (on `kill` or when the handle is dropped) closes the job
/// handle and terminates the whole process tree via `KILL_ON_JOB_CLOSE`.
pub struct TokioChild {
    inner: tokio::process::Child,
    pid: u32,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

#[cfg(windows)]
impl TokioChild {
    /// Terminate the whole tree: dropping the job closes its handle, and
    /// `KILL_ON_JOB_CLOSE` reaps every process in it. Written once so no stop
    /// arm can accidentally skip the job drop.
    async fn force(&mut self) -> Result<(), io::Error> {
        drop(self.job.take());
        self.inner.kill().await
    }
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

    /// On Windows, dropping the job closes its handle; with `KILL_ON_JOB_CLOSE`
    /// the OS terminates the whole tree (workers plus init-tool grandchildren).
    ///
    /// Windows cannot deliver a graceful *request* from safe Rust, so `protocol`
    /// selects between forcing now and letting an already-requested graceful
    /// stop finish. Under [`StopProtocol::MasterInterrupt`] the engine's own
    /// admin command has already been issued a layer up, so the child is given a
    /// bounded wait to exit on its own; every other combination forces
    /// immediately. Both arms end in the job-object teardown, so no worker can
    /// outlive the call either way.
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
            match crate::pure::windows_stop_action(signal, protocol) {
                crate::pure::WindowsStop::Force => self.force().await,
                crate::pure::WindowsStop::AwaitThenForce { grace } => {
                    match tokio::time::timeout(grace, self.inner.wait()).await {
                        // The master exited on its own, but a wedged worker or
                        // logger can still be sitting in the job, and the
                        // contract is that no worker outlives this call. Drop
                        // the job rather than calling `force`: `inner` has been
                        // reaped, and tokio's `kill` errors on an exited child.
                        Ok(_) => {
                            drop(self.job.take());
                            Ok(())
                        }
                        Err(_) => self.force().await,
                    }
                }
            }
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

#[cfg(all(test, windows))]
#[allow(clippy::unwrap_used, clippy::panic)]
mod windows_tests {
    use super::TokioProcessSpawner;
    use crate::error::ExitReason;
    use crate::traits::{ChildHandle, ProcessSpawner};
    use std::process::Command as StdCommand;

    #[tokio::test]
    async fn spawned_child_carries_a_job_and_reaps() {
        let mut cmd = StdCommand::new("cmd");
        cmd.args(["/C", "exit", "0"]);
        let mut child = TokioProcessSpawner.spawn(cmd).unwrap();
        assert!(
            child.job.is_some(),
            "a Windows child should be assigned to a job"
        );
        let reason = child.wait().await.unwrap();
        assert!(matches!(reason, ExitReason::Code(0)), "got {reason:?}");
    }
}
