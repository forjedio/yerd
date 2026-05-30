//! Idempotent tracing-subscriber installation.

use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry;

/// Install a compact stderr subscriber at the level implied by `verbose`.
///
/// At the default level the embedded DNS server (`hickory_server`) is capped at
/// `WARN`: it logs **every** inbound query at `INFO` (including the routine
/// `NXDomain` for non-`.test` names the OS resolver forwards here), which floods
/// the daemon log. Raising `verbose` lifts that cap so DNS traffic is visible
/// when debugging.
///
/// Idempotent: the `try_init` error is intentionally swallowed because
/// it fires only when a global subscriber has already been installed
/// (test re-entry, etc.) — the desired no-op.
pub fn init(verbose: u8) {
    let level = match verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    // hickory's per-request logging stays quiet at the default level; `-v`
    // (and above) lets it through with everything else.
    let hickory_level = if verbose == 0 {
        tracing::Level::WARN
    } else {
        level
    };
    let filter = Targets::new()
        .with_default(level)
        .with_target("hickory_server", hickory_level);
    let layer = fmt::layer().with_writer(std::io::stderr).compact();
    let _ = registry().with(layer.with_filter(filter)).try_init();
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
    fn init_is_idempotent() {
        init(0);
        init(1);
        init(2);
    }
}
