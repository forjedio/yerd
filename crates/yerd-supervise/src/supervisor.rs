//! Pure process-supervision state machine.
//!
//! All decisions live here. The driver (in each consumer's manager) does the I/O
//! (spawning, sleeping, probing, killing) and feeds events back into
//! [`transition`], which returns the next state plus a single [`Action`] for the
//! driver to execute.
//!
//! Time enters as [`Elapsed`] (a `Duration`) so tests can construct any state
//! without `Instant::now()`. The driver computes `Elapsed` against its own
//! `Instant` baseline before calling `transition`.
//!
//! ## Policy is an input, not a constant
//!
//! The timing/restart knobs ([`SupervisorPolicy`]) are supplied per call rather
//! than baked in as module constants. FPM pools start in well under a second and
//! are cheap to retry; database daemons can take tens of seconds to cold-boot
//! (redo-log init, crash recovery, fsync) and are expensive - and dangerous - to
//! kill and respawn mid-startup. Same state machine, different policy.
//!
//! The full transition table (and the policy decisions baked into it) lives in
//! [`transition`] below.

use std::time::Duration;

use crate::error::ExitReason;

/// Duration since the relevant state was entered.
///
/// Carried by `HealthCheckTick` and `StopTick` so the supervisor can compare
/// against the policy's health-check window / stop grace without peeking at the
/// wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed(pub Duration);

/// Timing and restart policy for the supervisor.
///
/// Supplied to [`transition`] and [`backoff_for`] per call. Two ready-made
/// profiles cover the current consumers: [`SupervisorPolicy::fpm`] (the original
/// FPM tuning) and [`SupervisorPolicy::database`] (slow cold-boot daemons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorPolicy {
    /// Maximum total time `Starting` may persist before a health-check timeout.
    pub health_check_window: Duration,
    /// First backoff wait (doubles each retry up to `backoff_max`).
    pub backoff_initial: Duration,
    /// Cap on backoff wait - exponential doubling saturates here.
    pub backoff_max: Duration,
    /// Consecutive failures before `PermanentFailure` is surfaced.
    pub max_restart_attempts: u32,
    /// Grace window between SIGTERM and SIGKILL.
    pub stop_grace: Duration,
}

impl SupervisorPolicy {
    /// Policy for PHP-FPM pools: fast to start, cheap to retry. These are the
    /// values that were hard-coded before policy was made an input, so the FPM
    /// path's behaviour is unchanged.
    #[must_use]
    pub const fn fpm() -> Self {
        Self {
            health_check_window: Duration::from_secs(5),
            backoff_initial: Duration::from_millis(100),
            backoff_max: Duration::from_secs(10),
            max_restart_attempts: 3,
            stop_grace: Duration::from_secs(2),
        }
    }

    /// Policy for database / cache daemons: slow cold-boot, expensive (and
    /// risky) to kill mid-startup. A generous readiness window avoids killing a
    /// healthy-but-slow server during `InnoDB` redo init / crash recovery; a
    /// longer stop grace lets the engine flush and shut down cleanly before
    /// SIGKILL.
    #[must_use]
    pub const fn database() -> Self {
        Self {
            health_check_window: Duration::from_secs(60),
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(10),
            max_restart_attempts: 3,
            stop_grace: Duration::from_secs(10),
        }
    }

    /// Policy for a Laravel Reverb app server: `php artisan reverb:start` boots
    /// the whole framework (composer autoload, container, config) before opening
    /// its socket, which on a cold opcache can take several seconds - so the
    /// readiness window is generous to avoid killing a healthy-but-slow start. It
    /// is a plain PHP process that drains on SIGTERM, so a short stop grace and a
    /// couple of restart attempts suffice.
    #[must_use]
    pub const fn reverb() -> Self {
        Self {
            health_check_window: Duration::from_secs(20),
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(10),
            max_restart_attempts: 3,
            stop_grace: Duration::from_secs(3),
        }
    }

    /// Policy for a `cloudflared` tunnel: a single outbound-only child whose
    /// readiness is the appearance of its public URL / edge-registration line in
    /// the logfile, which can take several seconds over a cold network. A
    /// generous readiness window avoids restart-looping a healthy-but-slow
    /// connect; `cloudflared` drains gracefully on SIGTERM so a short stop grace
    /// is enough.
    #[must_use]
    pub const fn tunnel() -> Self {
        Self {
            health_check_window: Duration::from_secs(60),
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(10),
            max_restart_attempts: 3,
            stop_grace: Duration::from_secs(5),
        }
    }
}

/// One supervised unit's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolState {
    /// No process; the driver hasn't been asked for one yet.
    Stopped,
    /// Spawn has been requested (or just succeeded); health-checking is in
    /// progress.
    Starting {
        /// Number of consecutive spawn attempts so far.
        attempts: u32,
        /// PID once `SpawnSucceeded` has been folded in; `None` between
        /// `EnsureRequested` / `BackoffElapsed` and `SpawnSucceeded`.
        pid: Option<u32>,
    },
    /// Healthy and accepting requests.
    Running {
        /// Process ID of the master.
        pid: u32,
    },
    /// Previous attempt(s) exited. The driver will retry per the backoff
    /// schedule unless `attempts >= policy.max_restart_attempts`.
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
    /// A health probe got a valid reply.
    HealthCheckOk,
    /// A probe attempt either timed out or got a connection error; driver hasn't
    /// seen a successful probe yet.
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
    /// Time since `Stopping` began. The driver feeds this once the grace timer
    /// elapses.
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
    /// Nothing - the driver should return to the caller (terminal state) or, in
    /// the non-terminal case, observe an invariant violation.
    None,
    /// Call the spawner to start the process.
    Spawn,
    /// Run one health probe attempt.
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
    /// The policy's health-check window elapsed without a healthy probe.
    HealthCheckTimedOut,
    /// The policy's `max_restart_attempts` consecutive failures.
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

/// How a *graceful* stop ([`KillSignal::Term`]) is delivered to a service's
/// process tree. A forced stop ([`KillSignal::Kill`]) always SIGKILLs the whole
/// process group regardless of this - at force time we want to reap stragglers.
///
/// This is a per-service delivery choice the driver makes; it is deliberately
/// NOT part of the FSM (which only ever decides graceful-vs-force) nor of
/// [`SupervisorPolicy`] (one policy is shared across all of a manager's
/// instances).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopProtocol {
    /// SIGTERM to the whole process group - reaps workers with the master.
    /// Correct for PHP-FPM, Redis, and `MySQL`/`MariaDB`.
    #[default]
    GroupTerm,
    /// SIGINT to the master PID only - Postgres "fast shutdown". The postmaster
    /// then shuts its own backends down. SIGTERM would be "smart shutdown" (it
    /// waits for clients and can hang), and signalling the whole group would
    /// mis-deliver to backends, where SIGINT means "cancel query".
    MasterInterrupt,
}

/// Backoff sleep for the `attempts`-th retry (1-indexed).
///
/// `min(policy.backoff_initial * 2^(attempts - 1), policy.backoff_max)`,
/// saturating. `attempts == 0` is treated as `1` (defensive - the state machine
/// should never produce a `Failed` with `attempts == 0`).
#[must_use]
pub fn backoff_for(attempts: u32, policy: &SupervisorPolicy) -> Duration {
    let n = attempts.max(1).saturating_sub(1);
    let factor: u64 = 1u64.checked_shl(n).unwrap_or(u64::MAX);
    let scaled = policy
        .backoff_initial
        .saturating_mul(u32::try_from(factor).unwrap_or(u32::MAX));
    scaled.min(policy.backoff_max)
}

/// Pure transition function.
///
/// Given the current state, an event, and the supervision `policy`, returns the
/// next state plus a single action for the driver. See the table in the module
/// docs.
#[must_use]
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
pub fn transition(
    state: PoolState,
    event: Event,
    policy: &SupervisorPolicy,
) -> (PoolState, Action) {
    match (state, event) {
        (PoolState::Stopped, Event::EnsureRequested) => (
            PoolState::Starting {
                attempts: 1,
                pid: None,
            },
            Action::Spawn,
        ),

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
        (PoolState::Starting { .. }, Event::HealthCheckOk) => (state, Action::None),
        (
            PoolState::Starting { .. },
            Event::HealthCheckTick {
                elapsed_since_starting,
            },
        ) if elapsed_since_starting.0 < policy.health_check_window => (state, Action::HealthCheck),
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
                wait: backoff_for(attempts, policy),
            },
        ),
        (PoolState::Starting { .. }, Event::StopRequested) => (
            PoolState::Stopping { sigkilled: false },
            Action::Kill {
                signal: KillSignal::Term,
            },
        ),

        (PoolState::Running { .. }, Event::Crashed { reason }) => (
            PoolState::Failed {
                last_exit: reason,
                attempts: 1,
            },
            Action::Backoff {
                wait: backoff_for(1, policy),
            },
        ),
        (PoolState::Running { .. }, Event::EnsureRequested) => (state, Action::None),
        (PoolState::Running { .. }, Event::StopRequested) => (
            PoolState::Stopping { sigkilled: false },
            Action::Kill {
                signal: KillSignal::Term,
            },
        ),

        (
            PoolState::Failed {
                last_exit,
                attempts,
            },
            Event::BackoffElapsed,
        ) => {
            if attempts < policy.max_restart_attempts {
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

        (
            PoolState::Stopping { sigkilled: false },
            Event::StopTick {
                elapsed_since_stopping,
            },
        ) if elapsed_since_stopping.0 >= policy.stop_grace => (
            PoolState::Stopping { sigkilled: true },
            Action::Kill {
                signal: KillSignal::Kill,
            },
        ),
        (PoolState::Stopping { sigkilled: true }, Event::StopTick { .. }) => (state, Action::None),
        (PoolState::Stopping { .. }, Event::StopComplete) => (PoolState::Stopped, Action::None),
        (PoolState::Stopping { .. }, Event::EnsureRequested) => (state, Action::None),

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
    fn backoff_for_table_fpm() {
        let p = SupervisorPolicy::fpm();
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
            assert_eq!(backoff_for(n, &p), want, "attempts={n}");
        }
    }

    #[test]
    fn policy_profiles_differ_as_documented() {
        let fpm = SupervisorPolicy::fpm();
        let db = SupervisorPolicy::database();
        assert!(db.health_check_window > fpm.health_check_window);
        assert!(db.stop_grace > fpm.stop_grace);
        assert_eq!(fpm.health_check_window, Duration::from_secs(5));
        assert_eq!(db.health_check_window, Duration::from_secs(60));
    }

    #[test]
    fn tunnel_policy_tolerates_slow_connect_with_short_stop() {
        let t = SupervisorPolicy::tunnel();
        let fpm = SupervisorPolicy::fpm();
        assert_eq!(t.health_check_window, Duration::from_secs(60));
        assert!(t.health_check_window > fpm.health_check_window);
        assert_eq!(t.stop_grace, Duration::from_secs(5));
        assert_eq!(t.max_restart_attempts, 3);
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed(10)
                },
                &t
            ),
            (starting(1, Some(42)), Action::HealthCheck)
        );
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

    // Each case asserts a single (from, event) → (to, action) transition, under
    // the FPM policy (the values these tests were written against).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn transitions_table() {
        let p = SupervisorPolicy::fpm();
        let r1 = ExitReason::Code(1);

        assert_eq!(
            transition(PoolState::Stopped, Event::EnsureRequested, &p),
            (starting(1, None), Action::Spawn)
        );

        assert_eq!(
            transition(starting(1, None), Event::SpawnSucceeded { pid: 42 }, &p),
            (starting(1, Some(42)), Action::HealthCheck)
        );
        assert_eq!(
            transition(starting(1, Some(42)), Event::HealthCheckOk, &p),
            (PoolState::Running { pid: 42 }, Action::None)
        );
        assert_eq!(
            transition(starting(1, None), Event::HealthCheckOk, &p),
            (starting(1, None), Action::None)
        );
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed_ms(100)
                },
                &p
            ),
            (starting(1, Some(42)), Action::HealthCheck)
        );
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed(6)
                },
                &p
            ),
            (
                starting(1, Some(42)),
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );
        assert_eq!(
            transition(starting(2, Some(42)), Event::Crashed { reason: r1 }, &p),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 2
                },
                Action::Backoff {
                    wait: backoff_for(2, &p)
                }
            )
        );
        assert_eq!(
            transition(starting(1, Some(42)), Event::StopRequested, &p),
            (
                PoolState::Stopping { sigkilled: false },
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );

        assert_eq!(
            transition(
                PoolState::Running { pid: 42 },
                Event::Crashed { reason: r1 },
                &p
            ),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Action::Backoff {
                    wait: backoff_for(1, &p)
                }
            )
        );
        assert_eq!(
            transition(PoolState::Running { pid: 42 }, Event::EnsureRequested, &p),
            (PoolState::Running { pid: 42 }, Action::None)
        );
        assert_eq!(
            transition(PoolState::Running { pid: 42 }, Event::StopRequested, &p),
            (
                PoolState::Stopping { sigkilled: false },
                Action::Kill {
                    signal: KillSignal::Term
                }
            )
        );

        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Event::BackoffElapsed,
                &p
            ),
            (starting(2, None), Action::Spawn)
        );
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: p.max_restart_attempts
                },
                Event::BackoffElapsed,
                &p
            ),
            (
                PoolState::Failed {
                    last_exit: r1,
                    attempts: p.max_restart_attempts
                },
                Action::EmitError(ErrorTag::PermanentFailure)
            )
        );
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: p.max_restart_attempts
                },
                Event::EnsureRequested,
                &p
            ),
            (starting(1, None), Action::Spawn)
        );
        assert_eq!(
            transition(
                PoolState::Failed {
                    last_exit: r1,
                    attempts: 1
                },
                Event::StopRequested,
                &p
            ),
            (PoolState::Stopped, Action::None)
        );

        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: false },
                Event::StopTick {
                    elapsed_since_stopping: elapsed(3)
                },
                &p
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
                },
                &p
            ),
            (PoolState::Stopping { sigkilled: false }, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: true },
                Event::StopTick {
                    elapsed_since_stopping: elapsed(10)
                },
                &p
            ),
            (PoolState::Stopping { sigkilled: true }, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: false },
                Event::StopComplete,
                &p
            ),
            (PoolState::Stopped, Action::None)
        );
        assert_eq!(
            transition(
                PoolState::Stopping { sigkilled: true },
                Event::EnsureRequested,
                &p
            ),
            (PoolState::Stopping { sigkilled: true }, Action::None)
        );
    }

    #[test]
    fn database_policy_tolerates_slow_startup() {
        let db = SupervisorPolicy::database();
        assert_eq!(
            transition(
                starting(1, Some(42)),
                Event::HealthCheckTick {
                    elapsed_since_starting: elapsed(6)
                },
                &db
            ),
            (starting(1, Some(42)), Action::HealthCheck)
        );
    }

    #[test]
    fn no_accidental_transitions() {
        let p = SupervisorPolicy::fpm();
        let (next, act) = transition(
            PoolState::Stopped,
            Event::HealthCheckTick {
                elapsed_since_starting: elapsed_ms(10),
            },
            &p,
        );
        assert_eq!(next, PoolState::Stopped);
        assert_eq!(act, Action::None);

        let (next, act) = transition(PoolState::Running { pid: 7 }, Event::BackoffElapsed, &p);
        assert_eq!(next, PoolState::Running { pid: 7 });
        assert_eq!(act, Action::None);
    }
}
