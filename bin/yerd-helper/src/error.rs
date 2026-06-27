//! Error type + sysexits.h exit-code mapping.
//!
//! `main.rs` is the only place that turns [`HelperError`] into an exit
//! code; everything else returns `Result<(), HelperError>`. The
//! [`exit_code`] mapping below is exhaustive - adding a new
//! [`HelperError`] variant without extending the match is a compile
//! error rather than a silent code change.

use std::path::PathBuf;

use thiserror::Error;
use yerd_platform::ArgvParseError;

/// Single error type for the helper binary.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HelperError {
    /// Argv structurally malformed (clap parse failure, unknown
    /// subcommand, etc.). Surfaces as `EX_USAGE` (64).
    #[error("argv usage: {0}")]
    ArgvUsage(String),

    /// Argv parsed structurally but a typed value (fingerprint, addr)
    /// was invalid. Surfaces as `EX_DATAERR` (65).
    #[error("argv data: {0}")]
    ArgvData(#[from] ArgvParseError),

    /// Effective UID is not 0. Daemon should retry under elevation.
    /// Surfaces as `EX_NOPERM` (77).
    #[error("not running privileged (effective uid != 0)")]
    NotPrivileged,

    /// This OS does not implement the requested operation. Surfaces as
    /// `EX_CONFIG` (78).
    #[error("operation not supported on this OS: {operation}")]
    Unsupported {
        /// Operation tag from `yerd_platform::error::ops`.
        operation: &'static str,
    },

    /// Defence-in-depth validator rejected an input. Surfaces as
    /// `EX_DATAERR` (65).
    #[error("validation: {reason}")]
    Validation {
        /// Specific validator that fired.
        reason: ValidationReason,
    },

    /// The PEM body's SHA-256 did not match the fingerprint argv said
    /// it would. Surfaces as `EX_DATAERR` (65).
    #[error("fingerprint mismatch: argv says {expected}, PEM hashes to {actual}")]
    FingerprintMismatch {
        /// Argv-claimed fingerprint, as 64-char lowercase hex.
        expected: String,
        /// Actual fingerprint computed from the PEM, as 64-char hex.
        actual: String,
    },

    /// I/O error against a known path. Surfaces as `EX_IOERR` (74).
    #[error("I/O at {path}: {source}", path = path.display())]
    Io {
        /// The path the I/O was directed at.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// External command spawn or non-zero exit.
    #[error("external command {tool} failed: {reason}")]
    Command {
        /// Basename of the external program.
        tool: &'static str,
        /// Specific failure.
        reason: CommandReason,
    },

    /// `from_argv` round-trip cross-check failed in a debug build. This
    /// is a wire-contract drift bug and surfaces as `EX_SOFTWARE` (70).
    #[error(
        "internal wire-contract drift: clap parsed {clap:?} but from_argv parsed {from_argv:?}"
    )]
    WireDrift {
        /// Argv operation tag clap produced.
        clap: &'static str,
        /// Argv operation tag `from_argv` produced.
        from_argv: &'static str,
    },
}

/// Specific failure modes for [`HelperError::Validation`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ValidationReason {
    /// Fingerprint argv was not exactly 64 lowercase hex chars.
    #[error("fingerprint must be exactly 64 lowercase hex characters")]
    BadFingerprintHex,
    /// Path must be absolute (we never canonicalise; relative paths
    /// resolve unpredictably under elevation).
    #[error("path must be absolute: {}", .0.display())]
    PathNotAbsolute(PathBuf),
    /// Path does not exist.
    #[error("path does not exist: {}", .0.display())]
    PathMissing(PathBuf),
    /// Path exists but is not a regular file.
    #[error("path is not a regular file: {}", .0.display())]
    PathNotFile(PathBuf),
    /// `setcap --binary` basename was not `yerdd`. Linux-only (`setcap` is
    /// unsupported elsewhere).
    #[cfg(target_os = "linux")]
    #[error("binary basename must be 'yerdd', got {0:?}")]
    BinaryNameUnexpected(String),
    /// TLD did not validate against `yerd_core::Tld`.
    #[error("tld invalid: {0}")]
    TldInvalid(String),
    /// PEM had ≠ 1 CERTIFICATE blocks.
    #[error("expected exactly 1 CERTIFICATE block, got {count}")]
    ExpectedSingleCertPem {
        /// Number of CERTIFICATE blocks found.
        count: usize,
    },
    /// PEM could not be parsed.
    #[error("PEM could not be parsed")]
    PemParseFailed,
    /// A cert matched the fingerprint to uninstall, but it is not yerd's CA
    /// (its Subject CN is not `yerd_core::CA_COMMON_NAME`). The helper refuses
    /// to remove a certificate it can't confirm yerd installed. Not gated:
    /// both the Linux and macOS uninstall paths can raise it.
    #[error("certificate is not yerd's CA (subject CN {found_cn:?}); refusing to remove it")]
    CertNotYerdOwned {
        /// The Subject CN actually found (if any), for diagnostics.
        found_cn: Option<String>,
    },
    /// No recognised Linux CA anchor directory present. Linux-only.
    #[cfg(target_os = "linux")]
    #[error("no recognised CA anchor directory")]
    NoAnchorDir,
    /// A port-redirect port argument was zero. macOS-only: the pf-redirect op
    /// that validates it is `#[cfg(target_os = "macos")]`, so the variant is
    /// gated to match - otherwise it is dead code on Linux/Windows.
    #[cfg(target_os = "macos")]
    #[error("port must be non-zero (flag {0})")]
    PortInvalid(&'static str),
}

/// Specific failure modes for [`HelperError::Command`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CommandReason {
    /// `Command::spawn` failed (typically because the tool is not on
    /// the pinned `PATH`).
    #[error("spawn failed: {0}")]
    Spawn(#[source] std::io::Error),
    /// Process exited with a non-zero status.
    #[error("exited {0}")]
    NonZero(i32),
    /// Process was killed by a signal.
    #[error("killed by signal")]
    Signal,
    /// Lookup against the pinned `PATH` returned `NotFound`.
    #[error("not on PATH")]
    NotFound,
}

/// Map [`HelperError`] to the `sysexits.h` exit code the daemon will
/// observe. The mapping is exhaustive; new variants must be added here
/// or compilation fails.
#[must_use]
pub fn exit_code(err: &HelperError) -> u8 {
    match err {
        HelperError::ArgvUsage(_) => 64,
        HelperError::ArgvData(_)
        | HelperError::Validation { .. }
        | HelperError::FingerprintMismatch { .. } => 65,
        HelperError::Command {
            reason: CommandReason::NotFound,
            ..
        } => 69,
        HelperError::WireDrift { .. } => 70,
        HelperError::Io { .. } => 74,
        HelperError::Command { .. } => 75,
        HelperError::NotPrivileged => 77,
        HelperError::Unsupported { .. } => 78,
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
    fn argv_usage_maps_to_64() {
        assert_eq!(exit_code(&HelperError::ArgvUsage("x".into())), 64);
    }

    #[test]
    fn argv_data_maps_to_65() {
        assert_eq!(exit_code(&HelperError::ArgvData(ArgvParseError::Empty)), 65);
    }

    #[test]
    fn validation_maps_to_65() {
        assert_eq!(
            exit_code(&HelperError::Validation {
                reason: ValidationReason::BadFingerprintHex
            }),
            65
        );
        assert_eq!(
            exit_code(&HelperError::Validation {
                reason: ValidationReason::CertNotYerdOwned { found_cn: None }
            }),
            65
        );
    }

    #[test]
    fn fingerprint_mismatch_maps_to_65() {
        assert_eq!(
            exit_code(&HelperError::FingerprintMismatch {
                expected: "a".into(),
                actual: "b".into(),
            }),
            65
        );
    }

    #[test]
    fn command_not_found_maps_to_69() {
        assert_eq!(
            exit_code(&HelperError::Command {
                tool: "x",
                reason: CommandReason::NotFound,
            }),
            69
        );
    }

    #[test]
    fn wire_drift_maps_to_70() {
        assert_eq!(
            exit_code(&HelperError::WireDrift {
                clap: "install-ca",
                from_argv: "uninstall-ca",
            }),
            70
        );
    }

    #[test]
    fn io_maps_to_74() {
        assert_eq!(
            exit_code(&HelperError::Io {
                path: PathBuf::from("/tmp/x"),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            }),
            74
        );
    }

    #[test]
    fn command_other_maps_to_75() {
        assert_eq!(
            exit_code(&HelperError::Command {
                tool: "x",
                reason: CommandReason::NonZero(2),
            }),
            75
        );
        assert_eq!(
            exit_code(&HelperError::Command {
                tool: "x",
                reason: CommandReason::Signal,
            }),
            75
        );
        assert_eq!(
            exit_code(&HelperError::Command {
                tool: "x",
                reason: CommandReason::Spawn(std::io::Error::from(std::io::ErrorKind::Other)),
            }),
            75
        );
    }

    #[test]
    fn not_privileged_maps_to_77() {
        assert_eq!(exit_code(&HelperError::NotPrivileged), 77);
    }

    #[test]
    fn unsupported_maps_to_78() {
        assert_eq!(
            exit_code(&HelperError::Unsupported {
                operation: "setcap"
            }),
            78
        );
    }

    #[test]
    fn validation_reason_displays_carry_input() {
        let r = ValidationReason::PathNotAbsolute(PathBuf::from("foo"));
        assert!(r.to_string().contains("foo"));
        #[cfg(target_os = "linux")]
        {
            let r = ValidationReason::BinaryNameUnexpected("zerdd".into());
            assert!(r.to_string().contains("zerdd"));
        }
        let r = ValidationReason::ExpectedSingleCertPem { count: 2 };
        assert!(r.to_string().contains('2'));
    }

    #[test]
    fn argv_data_conversion_from_argv_parse_error() {
        let err: HelperError = ArgvParseError::BadFingerprint.into();
        assert!(matches!(err, HelperError::ArgvData(_)));
        assert_eq!(exit_code(&err), 65);
    }
}
