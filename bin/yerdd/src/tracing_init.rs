//! Idempotent tracing-subscriber installation.

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry;

/// Install a compact stderr subscriber at the level implied by `verbose`.
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
    let layer = fmt::layer().with_writer(std::io::stderr).compact();
    let _ = registry()
        .with(layer.with_filter(LevelFilter::from_level(level)))
        .try_init();
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
