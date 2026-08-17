//! OS-independent teardown decisions the I/O adapter consumes.
//!
//! Un-gated and table-tested on every OS, so the Windows stop policy is
//! exercised by the Linux and macOS legs too rather than only where it runs.

use std::time::Duration;

use crate::supervisor::{KillSignal, StopProtocol};

/// How long a child whose engine-level graceful stop was already issued is given
/// to finish before termination is forced.
///
/// Half the database stop grace, so a wedged engine cannot double the overall
/// stop budget.
pub const GRACEFUL_EXIT_WAIT: Duration = Duration::from_secs(5);

/// What a Windows stop should do with a child.
///
/// Windows offers no way to *request* a graceful exit from safe Rust: the
/// console-control-event API is `unsafe` FFI with no safe wrapper, supervised
/// children set no creation flags and so do not lead their own process group,
/// and `taskkill` without `/F` posts `WM_CLOSE` to top-level windows these
/// engines do not own. So the only fidelity available here is restraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsStop {
    /// Terminate immediately through the job object.
    Force,
    /// Let an already-requested graceful stop finish, for at most `grace`, then
    /// force. This does not send anything gentler; the request was made a layer
    /// up by the engine's own admin command.
    AwaitThenForce {
        /// The bounded wait before forcing.
        grace: Duration,
    },
}

/// Choose the Windows stop action for a `(signal, protocol)` pair.
///
/// Only `Term` under [`StopProtocol::MasterInterrupt`] waits: that is the
/// combination the service manager uses after it has already asked the engine
/// to shut itself down, so truncating it would discard a clean shutdown that is
/// already in flight. Everything else forces immediately.
#[must_use]
pub fn windows_stop_action(signal: KillSignal, protocol: StopProtocol) -> WindowsStop {
    match (signal, protocol) {
        (KillSignal::Term, StopProtocol::MasterInterrupt) => WindowsStop::AwaitThenForce {
            grace: GRACEFUL_EXIT_WAIT,
        },
        (KillSignal::Kill, _) | (KillSignal::Term, StopProtocol::GroupTerm) => WindowsStop::Force,
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

    #[test]
    fn only_term_with_master_interrupt_waits() {
        let cases = [
            (
                KillSignal::Term,
                StopProtocol::MasterInterrupt,
                WindowsStop::AwaitThenForce {
                    grace: GRACEFUL_EXIT_WAIT,
                },
            ),
            (
                KillSignal::Term,
                StopProtocol::GroupTerm,
                WindowsStop::Force,
            ),
            (
                KillSignal::Kill,
                StopProtocol::MasterInterrupt,
                WindowsStop::Force,
            ),
            (
                KillSignal::Kill,
                StopProtocol::GroupTerm,
                WindowsStop::Force,
            ),
        ];
        for (signal, protocol, want) in cases {
            assert_eq!(
                windows_stop_action(signal, protocol),
                want,
                "{signal:?} + {protocol:?}"
            );
        }
    }

    /// A zero grace would make the waiting arm indistinguishable from forcing.
    #[test]
    fn the_grace_is_non_zero() {
        assert!(!GRACEFUL_EXIT_WAIT.is_zero());
    }
}
