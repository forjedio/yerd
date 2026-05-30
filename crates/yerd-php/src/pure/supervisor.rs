//! Pure FPM-pool state machine.
//!
//! All decisions live here. The driver in `manager.rs` does the I/O
//! (spawning, sleeping, probing, killing) and feeds events back into
//! [`transition`] which returns the next state plus a single
//! [`Action`] for the driver to execute.
//!
//! Time enters as [`Elapsed`] (a `Duration`) so tests can construct any
//! state without `Instant::now()`. The driver is responsible for
//! computing `Elapsed` against its own `Instant` baseline before calling
//! `transition`.
//!
//! The full transition table (and the policy decisions baked into it) lives
//! in [`transition`] below.

use std::time::Duration;

use crate::error::ExitReason;

/// Duration since the relevant state was entered.
///
/// Carried by `HealthCheckTick` and `StopTick` so the supervisor can
/// compare against `HEALTH_CHECK_WINDOW` / `STOP_GRACE` without
/// peeking at the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed(pub Duration);

/// One pool's supervision state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolState {
    /// No process; the driver hasn't been asked for one yet.
    Stopped,
    /// Spawn has been requested (or just succeeded); health-checking is
    /// in progress.
    Starting {
        /// Number of consecutive spawn attempts so far.
        attempts: u32,
        /// PID once `SpawnSucceeded` has been folded in; `None` between
        /// `EnsureRequested` / `BackoffElapsed` and `SpawnSucceeded`.
        pid: Option<u32>,
    },
    /// Healthy and accepting requests.
    Running {
        /// Process ID of the FPM master.
        pid: u32,
    },
    /// Previous attempt(s) exited. The driver will retry per the
    /// backoff schedule unless `attempts >= MAX_RESTART_ATTEMPTS`.
    Failed {
        /// Exit reason of the most recent attempt.
        last_exit: ExitReason,
        /// Number of consecutive failed attempts.
        attempts: u32,
    },
    /// A stop was requested; SIGTERM has been sent (and SIGKILL too if
    /// `sigkilled`).
    Stopping {
        /// `true` once `Kill` has been sent in addition to `Term`.
        sigkilled: bool,
    },
}

/// Events the driver feeds back into the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `ensure()` was called.
    EnsureRequested,
    /// The spawner returned a live child.
    SpawnSucceeded {
        /// PID of the new child.
        pid: u32,
    },
    /// A `FastCGI` probe got a valid reply.
    HealthCheckOk,
    /// A probe attempt either timed out or got a connection error;
    /// driver hasn't seen a successful probe yet.
    HealthCheckTick {
        /// Time since the current `Starting` state began.
        elapsed_since_starting: Elapsed,
    },
    /// The child exited.
    Crashed {
        /// How it exited.
        reason: ExitReason,
    },
    /// `stop()` was called.
    StopRequested,
    /// The child exited after a stop signal.
    StopComplete,
    /// Time since `Stopping` began. The driver feeds this once the grace
    /// timer elapses.
    StopTick {
        /// Time since the current `Stopping` state began.
        elapsed_since_stopping: Elapsed,
    },
    /// The backoff sleep elapsed.
    BackoffElapsed,
}

/// What the driver should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing — the driver should return to the caller (terminal state)
    /// or, in the non-terminal case, observe an invariant violation.
    None,
    /// Call the spawner to start FPM.
    Spawn,
    /// Run one `FastCGI` probe attempt.
    HealthCheck,
    /// Sleep this long, then feed `BackoffElapsed`.
    Backoff {
        /// How long to sleep.
        wait: Duration,
    },
    /// Send a signal to the child.
    Kill {
        /// Which signal to send.
        signal: KillSignal,
    },
    /// Surface a terminal error to the caller.
    EmitError(ErrorTag),
}

/// Which terminal error the driver should construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorTag {
    /// `HEALTH_CHECK_WINDOW` elapsed without a healthy probe.
    HealthCheckTimedOut,
    /// `MAX_RESTART_ATTEMPTS` consecutive failures.
    PermanentFailure,
}

/// Which signal `Action::Kill` requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSignal {
    /// SIGTERM (graceful). On Windows: maps to `Child::kill`.
    Term,
    /// SIGKILL (forced). On Windows: maps to `Child::kill`.
    Kill,
}

/// Maximum total time `Starting` may persist before health-check timeout.
pub const HEALTH_CHECK_WINDOW: Duration = Duration::from_secs(5);
/// First backoff wait.
pub const BACKOFF_INITIAL: Duration = Duration::from_millis(100);
/// Cap on backoff wait — exponential doubling saturates here.
pub const BACKOFF_MAX: Duration = Duration::from_secs(10);
/// Number of consecutive failures before `PermanentFailure` is surfaced.
pub const MAX_RESTART_ATTEMPTS: u32 = 3;
/// Grace window between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(2);

/// Backoff sleep for the `attempts`-th retry (1-indexed).
///
/// `min(BACKOFF_INITIAL * 2^(attempts - 1), BACKOFF_MAX)`, saturating.
/// `attempts == 0` is treated as `1` (defensive — the state machine
/// should never produce a `Failed` with `attempts == 0`).
#[must_use]
pub fn backoff_for(attempts: u32) -> Duration {
    let n = attempts.max(1).saturating_sub(1);
    // Saturating shift: anything past `u64::BITS - 1` saturates to `u64::MAX`,
    // then `min` clamps to `BACKOFF_MAX`. Use `u32` math.
    let factor: u64 = 1u64.checked_shl(n).unwrap_or(u64::MAX);
    let scaled = BACKOFF_INITIAL.saturating_mul(u32::try_from(factor).unwrap_or(u32::MAX));
    scaled.min(BACKOFF_MAX)
}

/// Pure transition function.
///
/// Given the current state and an event, returns the next state plus a
/// single action for the driver. See the table in the module docs.
#[must_use]
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
pub fn transition(state: PoolState, event: Event) -> (PoolState, Action) {
    match (state, event) {
        // -- Stopped --------------------------------------------------
        (PoolState::Stopped, Event::EnsureRequested) => (
            PoolState::Starting {
                attempts: 1,
                pid: None,
            },
            Action::Spawn,
        ),

        // -- Starting -------------------------------------------------
        (PoolState::Starting { attempts, .. }, Event::SpawnSucceeded { pid }) => (
            PoolState::Starting {
                attempts,
                pid: Some(pid),
            },
            Action::HealthCheck,
        ),
        (PoolState::Starting { pid: Some(pid), .. }, Event::HealthCheckOk) => {
            (PoolState::Running { pid }, Action::None)
        }
        (PoolState::Starting { .. }, Event::HealthCheckOk) => {
            // HealthCheckOk before SpawnSucceeded is an out-of-order event;
            // ignore.
            (state, Action::None)
        }
        (
            PoolState::Starting { .. },
            Event::HealthCheckTick {
                elapsed_since_starting,
            },
        ) if elapsed_since_starting.0 < HEALTH_CHECK_WINDOW => (state, Action::HealthCheck),
        (PoolState::Starting { .. }, Event::HealthCheckTick { .. }) => (
            state,
            Action::Kill {
                signal: KillSignal::Term,
            },
        ),
        (PoolState::Starting { attempts, .. }, Event::Crashed { reason }) => (
            PoolState::Failed {
                last_exit: reason,
                attempts,
            },
            Action::Backoff {
                wait: backoff_for(attempts),
            },
        ),
        (PoolState::Starting { .. }, Event::StopRequested) => (
            PoolState::Stopping { sigkilled: false },
            Action::Kill {
                signal: KillSignal::Term,
            },
        ),

        // -- Running --------------------------------------------------
        (PoolState::Running { .. }, Event::Crashed { reason }) => (
            PoolState::Failed {
                last_exit: reason,
                attempts: 1,
            },
            Action::Backoff {
                wait: backoff_for(1),
            },
        ),
        (PoolState::Running { .. }, Event::EnsureRequested) => (state, Action::None),
        (PoolState::Running { .. }, Event::StopRequested) => (
            PoolState::Stopping { sigkilled: false },
            Action::Kill {
                signal: KillSignal::Term,
            },
        ),

        // -- Failed ---------------------------------------------------
        (
            PoolState::Failed {
                last_exit,
                attempts,
            },
            Event::BackoffElapsed,
        ) => {
            if attempts < MAX_RESTART_ATTEMPTS {
                (
                    PoolState::Starting {
                        attempts: attempts + 1,
                        pid: None,
                    },
                    Action::Spawn,
                )
            } else {
                (
                    PoolState::Failed {
                        last_exit,
                        attempts,
                    },
                    Action::EmitError(ErrorTag::PermanentFailure),
                )
            }
        }
        (PoolState::Failed { .. }, Event::EnsureRequested) => (
            PoolState::Starting {
                attempts: 1,
                pid: None,
            },
            Action::Spawn,
        ),
        (PoolState::Failed { .. }, Event::StopRequested) => (PoolState::Stopped, Action::None),

        // -- Stopping -------------------------------------------------
        (
            PoolState::Stopping { sigkilled: false },
            Event::StopTick {
                elapsed_since_stopping,
            },
        ) if elapsed_since_stopping.0 >= STOP_GRACE => (
            PoolState::Stopping { sigkilled: true },
            Action::Kill {
                signal: KillSignal::Kill,
            },
        ),
        (PoolState::Stopping { sigkilled: true }, Event::StopTick { .. }) => (state, Action::None),
        (PoolState::Stopping { .. }, Event::StopComplete) => (PoolState::Stopped, Action::None),
        (PoolState::Stopping { .. }, Event::EnsureRequested) => (state, Action::None),

        // -- Catchall -------------------------------------------------
        _ => (state, Action::None),
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
    fn backoff_for_table() {
        let cases = [
            (1u32, Duration::from_millis(100)),
            (2, Duration::from_millis(200)),
            (3, Duration::from_millis(400)),
            (4, Duration::from_millis(800)),
            (5, Duration::from_millis(1600)),
            (6, Duration::from_millis(3200)),
            (7, Duration::from_millis(6400)),
            (8, Duration::from_secs(10)),
            (100, Duration::from_secs(10)),
        ];
        for (n, want) in cases {
            assert_eq!(backoff_for(n), want, "attempts={n}");
        }
    }

    fn elapsed(secs: u64) -> Elapsed {
        Elapsed(Duration::from_secs(secs))
    }

    fn elapsed_ms(ms: u64) -> Elapsed {
        Elapsed(Duration::from_millis(ms))
    }

    fn starting(attempts: u32, pid: Option<u32>) -> PoolState {
        PoolState::Starting { attempts, pid }
    }

    // Each case asserts a single (from, event) → (to, action) transition.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn transitions_table() {
        let r1 = ExitReason::Code(1);

        // Stopped
        assert_eq!(
            transition(PoolState::Stopped, Event::EnsureRequested),
            (starting(1, None), Action::Spawn)
        );

        // Starting
        assert_eq!(
            transition(starting(1, None), Event::SpawnSucceeded { pid: 42 }),
            (starting(1, Some(42)), Action::HealthCheck)
        );
        assert_eq!(
            transition(starting(1, Some(42)), Event::HealthCheckOk),
            (PoolState::Running { pid: 42 }, Action::None)
        );
        // HealthCheckOk before SpawnSucceeded — out of order, ignored.
        assert_eq!(
            transition(starting(1, None), Event::HealthCheckOk),
            (starting(1, None), Action::None)
        );
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed_ms(100)
                }
            ),
            (starting(1, Some(42)), Action::HealthCheck)
        );
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed(6)
                }
            ),
            (
                starting(1, Some(42)),
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );
        assert_eq!(
            transition(starting(2, Some(42)), Event::Crashed { reason: r1 }),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 2
                },
                Action::Backoff {
                    wait: backoff_for(2)
                }
            )
        );
        assert_eq!(
            transition(starting(1, Some(42)), Event::StopRequested),
            (
                PoolState::Stopping { sigkilled: false },
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );

        // Running
        assert_eq!(
            transition(
                PoolState::Running { pid: 42 },
                Event::Crashed { reason: r1 }
            ),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Action::Backoff {
                    wait: backoff_for(1)
                }
            )
        );
        assert_eq!(
            transition(PoolState::Running { pid: 42 }, Event::EnsureRequested),
            (PoolState::Running { pid: 42 }, Action::None)
        );
        assert_eq!(
            transition(PoolState::Running { pid: 42 }, Event::StopRequested),
            (
                PoolState::Stopping { sigkilled: false },
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );

        // Failed — under MAX
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Event::BackoffElapsed
            ),
            (starting(2, None), Action::Spawn)
        );
        // Failed — at MAX
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: MAX_RESTART_ATTEMPTS
                },
                Event::BackoffElapsed
            ),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: MAX_RESTART_ATTEMPTS
                },
                Action::EmitError(ErrorTag::PermanentFailure)
            )
        );
        // Operator restart resets budget
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: MAX_RESTART_ATTEMPTS
                },
                Event::EnsureRequested
            ),
            (starting(1, None), Action::Spawn)
        );
        // Stop from Failed is immediate
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Event::StopRequested
            ),
            (PoolState::Stopped, Action::None)
        );

        // Stopping
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: false },
                Event::StopTick {
                    elapsed_since_stopping: elapsed(3)
                }
            ),
            (
                PoolState::Stopping { sigkilled: true },
                Action::Kill {
                    signal: KillSignal::Kill
                }
            )
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: false },
                Event::StopTick {
                    elapsed_since_stopping: elapsed_ms(500)
                }
            ),
            (PoolState::Stopping { sigkilled: false }, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: true },
                Event::StopTick {
                    elapsed_since_stopping: elapsed(10)
                }
            ),
            (PoolState::Stopping { sigkilled: true }, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: false },
                Event::StopComplete
            ),
            (PoolState::Stopped, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: true },
                Event::EnsureRequested
            ),
            (PoolState::Stopping { sigkilled: true }, Action::None)
        );
    }

    #[test]
    fn no_accidental_transitions() {
        // Stopped + HealthCheckTick: catchall.
        let (next, act) = transition(
            PoolState::Stopped,
            Event::HealthCheckTick {
                elapsed_since_starting: elapsed_ms(10),
            },
        );
        assert_eq!(next, PoolState::Stopped);
        assert_eq!(act, Action::None);

        // Running + BackoffElapsed: catchall.
        let (next, act) = transition(PoolState::Running { pid: 7 }, Event::BackoffElapsed);
        assert_eq!(next, PoolState::Running { pid: 7 });
        assert_eq!(act, Action::None);
    }
}
