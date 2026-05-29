//! Error types for `yerd-php`.
//!
//! [`PhpError`] is **not** `Clone + Eq` because it wraps `std::io::Error` and
//! `yerd_platform::PlatformError`. This mirrors `yerd-config::ConfigError`'s
//! shape — see plan §6. The daemon is the only consumer; if a GUI surface
//! ever needs a `Clone + Eq` shadow, add one then.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use thiserror::Error;
use yerd_core::PhpVersion;

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

/// Error returned by [`crate::traits::Downloader::download`].
///
/// Carries a flattened message rather than wrapping a transport type so that
/// test fakes can construct it without pulling in `reqwest`, and so the public
/// surface stays transport-agnostic. SHA-256 verification of the fetched bytes
/// is the caller's responsibility, not the downloader's.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// The transfer failed — connection, TLS, timeout, or a non-success HTTP
    /// status.
    #[error("download failed for {url}: {reason}")]
    Transport {
        /// The URL that failed to download.
        url: String,
        /// Flattened underlying error.
        reason: String,
    },
}

/// Classification of an `io::Error` returned by `ProcessSpawner::spawn` or
/// by `ChildHandle::wait` during supervision.
///
/// Surfaces the most common operational mistakes (missing binary, denied
/// permissions) without forcing callers to re-inspect the underlying
/// `io::ErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnFailureReason {
    /// `io::ErrorKind::NotFound` — usually a missing FPM binary path.
    BinaryNotFound,
    /// `io::ErrorKind::PermissionDenied` — FPM binary present but not
    /// executable, or denied by a security policy.
    PermissionDenied,
    /// The child was alive but `child.wait()` failed mid-supervision.
    WaitFailed,
    /// Any other `io::ErrorKind`.
    Other,
}

impl SpawnFailureReason {
    /// Classify an `io::ErrorKind` for surfacing in `PhpError::Spawn`.
    #[must_use]
    pub(crate) fn from_kind(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::NotFound => Self::BinaryNotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Other,
        }
    }
}

/// How a supervised FPM child exited.
///
/// Implements `Hash` so callers can aggregate by exit reason for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExitReason {
    /// Normal exit with this OS-level code.
    Code(i32),
    /// Unix-only: killed by signal `n`. On Windows this is never produced
    /// in production code; tests may construct it directly.
    Signal(i32),
    /// The exit status carried no code and no signal (shouldn't normally
    /// happen but is a defined fallback).
    Unknown,
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(c) => write!(f, "exit {c}"),
            Self::Signal(s) => write!(f, "signal {s}"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

impl ExitReason {
    /// Translate a `std::process::ExitStatus` into the crate's vocabulary.
    pub(crate) fn from_status(status: ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = status.signal() {
                return Self::Signal(sig);
            }
        }
        if let Some(code) = status.code() {
            Self::Code(code)
        } else {
            Self::Unknown
        }
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
    use std::collections::HashSet;

    fn io_err() -> io::Error {
        io::Error::from(io::ErrorKind::AddrInUse)
    }

    #[test]
    fn exit_reason_display() {
        assert_eq!(ExitReason::Code(0).to_string(), "exit 0");
        assert_eq!(ExitReason::Code(137).to_string(), "exit 137");
        assert_eq!(ExitReason::Signal(9).to_string(), "signal 9");
        assert_eq!(ExitReason::Unknown.to_string(), "unknown");
    }

    #[test]
    fn exit_reason_hashable() {
        let mut set = HashSet::new();
        set.insert(ExitReason::Code(0));
        set.insert(ExitReason::Code(1));
        set.insert(ExitReason::Signal(9));
        assert_eq!(set.len(), 3);
        assert!(set.contains(&ExitReason::Code(0)));
    }

    #[test]
    fn spawn_failure_reason_from_kind() {
        assert_eq!(
            SpawnFailureReason::from_kind(io::ErrorKind::NotFound),
            SpawnFailureReason::BinaryNotFound
        );
        assert_eq!(
            SpawnFailureReason::from_kind(io::ErrorKind::PermissionDenied),
            SpawnFailureReason::PermissionDenied
        );
        assert_eq!(
            SpawnFailureReason::from_kind(io::ErrorKind::AddrInUse),
            SpawnFailureReason::Other
        );
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
