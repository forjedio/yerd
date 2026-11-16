//! Error type for tunnel supervision.

use yerd_supervise::ExitReason;

/// Failures from supervising a `cloudflared` tunnel.
///
/// Pure validation never fails today (origin/arg/config generation is total), so
/// these are all runtime failures from the async manager (the I/O edge). Spawn
/// failures inside supervision are not returned as errors: the manager records
/// them as the instance's `last_error` and retries with backoff, since a tunnel
/// never gives up.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    /// The `cloudflared` binary has not been installed yet.
    #[error("cloudflared is not installed")]
    NotInstalled,

    /// Spawning the `cloudflared` process failed.
    #[error("failed to spawn cloudflared: {0}")]
    Spawn(#[source] std::io::Error),

    /// Waiting on / signalling the child failed.
    #[error("cloudflared process I/O failed: {0}")]
    Io(#[source] std::io::Error),

    /// The readiness window elapsed before the tunnel came up (no URL / no edge
    /// registration line appeared).
    #[error("tunnel for {site} did not become ready in time")]
    ReadinessTimedOut {
        /// The site whose tunnel timed out.
        site: String,
    },

    /// The manager has been shut down, so no new `cloudflared` may be spawned
    /// (a late start would otherwise outlive supervision).
    #[error("tunnel manager is shutting down")]
    ShuttingDown,

    /// The tunnel exited repeatedly and exhausted its restart budget.
    #[error("tunnel for {site} failed permanently (last exit: {last_exit})")]
    PermanentFailure {
        /// The site whose tunnel failed.
        site: String,
        /// Exit reason of the final attempt.
        last_exit: ExitReason,
    },
}
