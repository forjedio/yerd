//! Idempotent tracing-subscriber installation.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry;

/// Install the tracing subscriber: a compact stderr layer (always) plus, when
/// `log_dir` is given, a daily-rolling file layer at `{log_dir}/yerdd.<date>.log`.
///
/// The file layer is the load-bearing one: under launchd/systemd the daemon's
/// stderr is discarded, so a durable log is the only way a start failure leaves
/// a trace the GUI's diagnostics can surface.
///
/// At the default level the embedded DNS server (`hickory_server`) is capped at
/// `WARN`: it logs **every** inbound query at `INFO` (including the routine
/// `NXDomain` for non-`.test` names the OS resolver forwards here), which floods
/// the daemon log. Raising `verbose` lifts that cap so DNS traffic is visible
/// when debugging. The cap is shared by both layers (the filter is cloned).
///
/// Returns the file appender's [`WorkerGuard`] when a file layer was installed;
/// the caller **must keep it alive** for the lifetime of the process - dropping
/// it stops the background flush worker and log lines are lost. Returns `None`
/// when `log_dir` is `None` (unit tests, or dirs couldn't be resolved) or when
/// the rolling appender could not be built.
///
/// Idempotency: with `log_dir = None` this is idempotent - the `try_init` error
/// is swallowed because it fires only when a global subscriber already exists
/// (test re-entry), the desired no-op. With `log_dir = Some(..)` it is **not**
/// idempotent (the appender + its worker are created before `try_init`), which
/// is fine: `main` calls it exactly once.
#[must_use]
pub fn init(verbose: u8, log_dir: Option<&Path>) -> Option<WorkerGuard> {
    let level = match verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    let hickory_level = if verbose == 0 {
        tracing::Level::WARN
    } else {
        level
    };
    let filter = Targets::new()
        .with_default(level)
        .with_target("hickory_server", hickory_level);

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .compact()
        .with_filter(filter.clone());

    let (file_layer, guard) = match log_dir {
        Some(dir) => match build_file_appender(dir) {
            Ok(appender) => {
                let (writer, guard) = tracing_appender::non_blocking(appender);
                let layer = fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer)
                    .with_filter(filter);
                (Some(layer), Some(guard))
            }
            Err(e) => {
                eprintln!(
                    "yerdd: could not open log file in {}: {e}; logging to stderr only",
                    dir.display()
                );
                (None, None)
            }
        },
        None => (None, None),
    };

    let _ = registry().with(stderr_layer).with(file_layer).try_init();
    guard
}

/// Build the daily-rolling `yerdd.<date>.log` appender, keeping at most a few
/// days so the log can't grow without bound (this is a dev tool, not a server).
fn build_file_appender(
    dir: &Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(3)
        .filename_prefix("yerdd")
        .filename_suffix("log")
        .build(dir)
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
        assert!(init(0, None).is_none());
        assert!(init(1, None).is_none());
        assert!(init(2, None).is_none());
    }
}
