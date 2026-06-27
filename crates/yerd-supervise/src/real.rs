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

/// Spawns commands via `tokio::process::Command`, sets `kill_on_drop(true)` so
/// unexpected crashes of the daemon take the child down with them.
pub struct TokioProcessSpawner;

impl ProcessSpawner for TokioProcessSpawner {
    type Child = TokioChild;

    fn spawn(&self, cmd: StdCommand) -> Result<TokioChild, io::Error> {
        let mut tokio_cmd = tokio::process::Command::from(cmd);
        tokio_cmd.kill_on_drop(true);
        let child = tokio_cmd.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("child has no pid"))?;
        Ok(TokioChild { inner: child, pid })
    }
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
