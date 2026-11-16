//! `ProcessTable` trait - best-effort process inspection and signalling.
//!
//! Used to reap processes a previous daemon run left behind (for example a
//! `cloudflared` orphaned by a daemon crash). Like [`crate::SystemMetrics`] it
//! is best-effort and infallible at the type level: a process that is gone,
//! unreadable, or on an unsupported OS simply reads as absent. The caller owns
//! the decision of *which* processes to signal; the OS impls only read and
//! signal.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Read and signal processes by PID.
pub trait ProcessTable {
    /// The full command line of `pid`, arguments joined by single spaces.
    ///
    /// `None` when the process is gone, unreadable, or the OS is unsupported.
    fn command_line(&self, pid: u32) -> Option<String>;

    /// Signal the process group led by `pid`: SIGTERM, or SIGKILL when `force`.
    ///
    /// Refuses pid 0, pid 1, and any pid that does not fit an `i32` without
    /// signalling anything. Failures are ignored.
    fn signal_group(&self, pid: u32, force: bool);

    /// Whether `pid` still exists.
    fn is_alive(&self, pid: u32) -> bool;
}

/// A signal [`FakeProcessTable`] recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedSignal {
    /// The group leader PID that was signalled.
    pub pid: u32,
    /// Whether the signal was SIGKILL rather than SIGTERM.
    pub force: bool,
}

/// Test fake: a fixed table of command lines. A process stays alive until it
/// is signalled, except for PIDs marked stubborn, which survive SIGTERM and only
/// die on a forced signal.
#[derive(Debug, Default)]
pub struct FakeProcessTable {
    inner: Mutex<FakeInner>,
}

#[derive(Debug, Default)]
struct FakeInner {
    commands: BTreeMap<u32, String>,
    stubborn: BTreeSet<u32>,
    signals: Vec<RecordedSignal>,
}

impl FakeProcessTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a live process with this command line.
    #[must_use]
    pub fn with_process(self, pid: u32, command_line: &str) -> Self {
        self.guard().commands.insert(pid, command_line.to_owned());
        self
    }

    /// Add a live process that ignores SIGTERM.
    #[must_use]
    pub fn with_stubborn_process(self, pid: u32, command_line: &str) -> Self {
        {
            let mut inner = self.guard();
            inner.commands.insert(pid, command_line.to_owned());
            inner.stubborn.insert(pid);
        }
        self
    }

    /// Every signal sent, in order.
    #[must_use]
    pub fn signals(&self) -> Vec<RecordedSignal> {
        self.guard().signals.clone()
    }

    /// A poisoned lock still holds the recorded state, so recover rather than
    /// propagate: this is a test double, not a correctness boundary.
    fn guard(&self) -> MutexGuard<'_, FakeInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ProcessTable for FakeProcessTable {
    fn command_line(&self, pid: u32) -> Option<String> {
        self.guard().commands.get(&pid).cloned()
    }

    fn signal_group(&self, pid: u32, force: bool) {
        if crate::pure::process_signal::signal_target(pid).is_none() {
            return;
        }
        let mut inner = self.guard();
        inner.signals.push(RecordedSignal { pid, force });
        if force || !inner.stubborn.contains(&pid) {
            inner.commands.remove(&pid);
        }
    }

    fn is_alive(&self, pid: u32) -> bool {
        self.guard().commands.contains_key(&pid)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn fake_kills_on_term_unless_stubborn() {
        let table = FakeProcessTable::new()
            .with_process(10, "a")
            .with_stubborn_process(11, "b");

        table.signal_group(10, false);
        table.signal_group(11, false);

        assert!(!table.is_alive(10));
        assert!(table.is_alive(11));
        table.signal_group(11, true);
        assert!(!table.is_alive(11));
        assert_eq!(table.signals().len(), 3);
    }

    #[test]
    fn fake_refuses_reserved_pids() {
        let table = FakeProcessTable::new().with_process(1, "init");

        table.signal_group(1, true);
        table.signal_group(0, true);

        assert!(table.signals().is_empty());
        assert!(table.is_alive(1));
    }
}
