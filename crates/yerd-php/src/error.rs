//! Error types for `yerd-php`.
//!
//! [`PhpError`] is **not** `Clone + Eq` because it wraps `std::io::Error` and
//! `yerd_platform::PlatformError`. This mirrors `yerd-config::ConfigError`'s
//! shape. The daemon is the only consumer; if a GUI surface ever needs a
//! `Clone + Eq` shadow, add one then.

use std::io;
use std::path::PathBuf;

use thiserror::Error;
use yerd_core::PhpVersion;

// Process-agnostic outcome/error types moved to `yerd-supervise`; re-exported so
// `crate::error::*` paths and the `yerd_php` public API are unchanged.
pub use yerd_supervise::{DownloadError, ExitReason, SpawnFailureReason};

/// Errors produced by `yerd-php`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PhpError {
    /// The requested PHP version is not installed (not in the manager's
    /// `binaries` map).
    #[error("PHP version {version} is not installed")]
    VersionNotInstalled {
        /// The version that was requested.
        version: PhpVersion,
    },

    /// `read_dir` of the bundled-PHP root directory failed for a reason
    /// other than `NotFound`.
    #[error("scan {} for bundled PHP: {source}", dir.display())]
    DiscoveryIo {
        /// The directory we were scanning.
        dir: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Spawning the FPM child process failed (or the wait failed while
    /// supervising — see [`SpawnFailureReason::WaitFailed`]).
    #[error("spawn FPM for {version} ({reason:?}): {source}")]
    Spawn {
        /// Which version's FPM we tried to spawn.
        version: PhpVersion,
        /// Classification of the underlying `io::Error`.
        reason: SpawnFailureReason,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Writing the FPM config file (`tempfile + rename`) failed.
    #[error("write FPM config to {}: {source}", path.display())]
    ConfigWrite {
        /// The config path we were trying to write.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// `HEALTH_CHECK_WINDOW` elapsed without an OK probe response. The FPM
    /// child has been killed before this error surfaces.
    #[error("health check for {version} timed out after {attempts} attempts")]
    HealthCheckTimedOut {
        /// Which version was being health-checked.
        version: PhpVersion,
        /// How many `Starting` attempts had accumulated.
        attempts: u32,
    },

    /// FPM crashed repeatedly past the restart budget.
    #[error("FPM for {version} crashed repeatedly (last exit: {reason})")]
    PermanentFailure {
        /// Which version exhausted its restart budget.
        version: PhpVersion,
        /// The most recent exit reason recorded by the supervisor.
        reason: ExitReason,
    },

    /// Allocating the FPM listen address failed (Windows-only path; the
    /// Unix planner does no binding).
    #[error("allocate listen port: {source}")]
    Bind {
        /// Underlying platform error.
        #[source]
        source: yerd_platform::PlatformError,
    },

    /// Sending a signal to the FPM child failed.
    #[error("kill FPM for {version}: {source}")]
    Kill {
        /// Which version's FPM we tried to signal.
        version: PhpVersion,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The running OS/architecture has no prebuilt PHP build.
    #[error("unsupported platform: {detail}")]
    UnsupportedPlatform {
        /// Which dimension is unsupported.
        detail: String,
    },

    /// No prebuilt build of the requested version is published for this
    /// platform (discovered from the distribution's listing).
    #[error("no prebuilt PHP {version} found for this platform at the distribution")]
    VersionUnavailable {
        /// The version that was requested.
        version: PhpVersion,
    },

    /// Downloading an artifact failed.
    #[error(transparent)]
    Download(#[from] DownloadError),

    /// Unpacking a downloaded archive failed (bad/empty/unsafe tar, or write).
    #[error("extract {what}: {detail}")]
    Extract {
        /// What we were extracting (e.g. the artifact URL).
        what: String,
        /// Human-readable failure detail.
        detail: String,
    },
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

    fn io_err() -> io::Error {
        io::Error::from(io::ErrorKind::AddrInUse)
    }

    #[test]
    fn php_error_display_pins_per_variant() {
        let v = PhpVersion::new(8, 3);

        let e = PhpError::VersionNotInstalled { version: v };
        let s = e.to_string();
        assert!(s.contains("not installed"), "got: {s}");
        assert!(s.contains("8.3"), "got: {s}");

        let e = PhpError::DiscoveryIo {
            dir: PathBuf::from("/tmp/missing"),
            source: io_err(),
        };
        let s = e.to_string();
        assert!(s.contains("scan"), "got: {s}");
        assert!(s.contains("/tmp/missing"), "got: {s}");

        let e = PhpError::Spawn {
            version: v,
            reason: SpawnFailureReason::BinaryNotFound,
            source: io_err(),
        };
        let s = e.to_string();
        assert!(s.contains("spawn FPM"), "got: {s}");
        assert!(s.contains("BinaryNotFound"), "got: {s}");

        let e = PhpError::ConfigWrite {
            path: PathBuf::from("/etc/php-fpm.conf"),
            source: io_err(),
        };
        assert!(e.to_string().contains("write FPM config"));

        let e = PhpError::HealthCheckTimedOut {
            version: v,
            attempts: 3,
        };
        let s = e.to_string();
        assert!(s.contains("health check"), "got: {s}");
        assert!(s.contains("3 attempts"), "got: {s}");

        let e = PhpError::PermanentFailure {
            version: v,
            reason: ExitReason::Code(1),
        };
        let s = e.to_string();
        assert!(s.contains("crashed repeatedly"), "got: {s}");
        assert!(s.contains("exit 1"), "got: {s}");

        let e = PhpError::Kill {
            version: v,
            source: io_err(),
        };
        assert!(e.to_string().contains("kill FPM"));
    }
}
