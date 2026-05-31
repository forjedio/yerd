//! Process-agnostic error/outcome types shared by the supervisor and its
//! consumers.
//!
//! These are deliberately free of any program-specific vocabulary (no
//! `PhpVersion`, no service id): [`ExitReason`] classifies how a supervised
//! child exited, [`SpawnFailureReason`] classifies why a spawn/wait failed, and
//! [`DownloadError`] is the transport-agnostic error of [`crate::Downloader`].

use std::fmt;
use std::io;
use std::process::ExitStatus;

use thiserror::Error;

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

/// Classification of an `io::Error` returned by
/// [`crate::traits::ProcessSpawner::spawn`] or by
/// [`crate::traits::ChildHandle::wait`] during supervision.
///
/// Surfaces the most common operational mistakes (missing binary, denied
/// permissions) without forcing callers to re-inspect the underlying
/// `io::ErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnFailureReason {
    /// `io::ErrorKind::NotFound` — usually a missing binary path.
    BinaryNotFound,
    /// `io::ErrorKind::PermissionDenied` — binary present but not executable,
    /// or denied by a security policy.
    PermissionDenied,
    /// The child was alive but `child.wait()` failed mid-supervision.
    WaitFailed,
    /// Any other `io::ErrorKind`.
    Other,
}

impl SpawnFailureReason {
    /// Classify an `io::ErrorKind` for surfacing in a caller's spawn error.
    #[must_use]
    pub fn from_kind(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::NotFound => Self::BinaryNotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Other,
        }
    }
}

/// How a supervised child exited.
///
/// Implements `Hash` so callers can aggregate by exit reason for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExitReason {
    /// Normal exit with this OS-level code.
    Code(i32),
    /// Unix-only: killed by signal `n`. On Windows this is never produced in
    /// production code; tests may construct it directly.
    Signal(i32),
    /// The exit status carried no code and no signal (shouldn't normally happen
    /// but is a defined fallback).
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
}
