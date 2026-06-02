//! Process-agnostic supervision substrate for Yerd.
//!
//! This crate holds the parts of process supervision that are not specific to
//! any particular supervised program: the trait seams the supervisor depends on
//! ([`ProcessSpawner`], [`ChildHandle`], [`Clock`], [`HealthProbe`],
//! [`Downloader`]), the production tokio-backed implementations of the
//! infrastructure traits ([`SystemClock`], [`TokioProcessSpawner`]), the generic
//! [`Listen`] address, and the **pure** supervision state machine
//! ([`supervisor`]).
//!
//! It is consumed by both `yerd-php` (FPM pools) and `yerd-services` (database /
//! cache daemons). The state machine's timing/restart policy is **not** baked in
//! — it is supplied per call via [`supervisor::SupervisorPolicy`], so an FPM pool
//! (fast to start, cheap to retry) and a database (slow cold-boot, expensive to
//! retry) can drive the same logic with different policies.
//!
//! Depends on nothing internal — it sits at the bottom of the crate graph next
//! to `yerd-core`.

#![forbid(unsafe_code)]

pub mod error;
pub mod listen;
pub mod real;
pub mod supervisor;
pub mod traits;

pub use error::{DownloadError, ExitReason, SpawnFailureReason};
pub use listen::Listen;
pub use real::{SystemClock, TokioChild, TokioProcessSpawner};
pub use supervisor::{
    backoff_for, transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
pub use traits::{ChildHandle, Clock, Downloader, HealthProbe, ProcessSpawner};

// Compile-time `Send + 'static` guard for the production infrastructure impls.
const _: () = {
    const fn assert_send_static<T: Send + Sync + 'static>() {}
    assert_send_static::<TokioProcessSpawner>();
    assert_send_static::<SystemClock>();
};
