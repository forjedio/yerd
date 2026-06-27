//! Error types for `yerd-config`.
//!
//! [`ConfigError`] is the single error type exposed by every fallible public
//! API in this crate. Each non-foreign variant carries a typed `*Reason`
//! sub-enum so callers can match on precise failure modes without parsing
//! message strings.
//!
//! Every public error enum carries `#[non_exhaustive]` so additions are
//! semver-compatible.

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by `yerd-config`.
///
/// Not `Clone` / `Eq`: wraps `toml::de::Error`, `toml::ser::Error`, and
/// `std::io::Error`. Matches `yerd-ipc::IpcError` in that respect. Unlike
/// `yerd_ipc::IpcError::Io { kind: io::ErrorKind }` (which stores the kind
/// to preserve `Eq`), this crate stores the full `io::Error` and a
/// [`PathBuf`] because diagnostic detail matters for `load`/`save`
/// debugging.
///
/// Construction of every variant happens inside this crate only.
/// `#[non_exhaustive]` blocks external construction of new variants but
/// does not block external field access on existing variants.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// TOML failed to lex/parse syntactically, or a per-field validator in
    /// `yerd-core` (via `serde::de::Error::custom`) rejected a domain value
    /// during deserialisation of the wire mirror.
    #[error("could not parse TOML: {0}")]
    Parse(#[from] toml::de::Error),

    /// TOML serialisation failed (always a bug - types in this crate must
    /// serialise cleanly).
    #[error("could not serialise TOML: {0}")]
    Serialize(#[from] toml::ser::Error),

    /// Cross-field or container-content invariant failed.
    #[error("config failed validation: {reason}")]
    Validate {
        /// Specific validation failure.
        reason: ValidateErrorReason,
    },

    /// A `yerd-core` validation failure surfaced when converting the parsed
    /// wire mirror into typed domain values (TLD, `PhpVersion`, `Site`).
    #[error("invalid domain value in config: {0}")]
    Core(#[from] yerd_core::CoreError),

    /// On-disk schema version is incompatible with `crate::CURRENT_VERSION`.
    /// Most commonly fired when `found > current`; also reachable if a
    /// `migrate::STEPS` misconfiguration leaves the version below current
    /// after `up()` returns.
    #[error("config schema version {found} is incompatible with supported version {current}")]
    UnsupportedVersion {
        /// The version found in the on-disk file.
        found: u32,
        /// The version this build of `yerd-config` supports
        /// (`crate::CURRENT_VERSION`).
        current: u32,
    },

    /// A forward migration failed.
    #[error("migration failed: {reason}")]
    Migration {
        /// Specific migration failure.
        reason: MigrationErrorReason,
    },

    /// I/O failed during [`crate::Config::load`] or [`crate::Config::save`].
    #[error("config I/O failed at {}: {source}", path.display())]
    Io {
        /// The destination/source path the caller passed (preserved as
        /// `PathBuf` so non-UTF-8 components on Windows survive into the
        /// error report).
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Specific failure modes for [`crate::Config::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidateErrorReason {
    /// Two linked sites share a `name()`.
    DuplicateLinkedSite,
    /// `ports.http == ports.https`.
    HttpHttpsPortsEqual,
    /// `ports.http == 0`.
    HttpPortZero,
    /// `ports.https == 0`.
    HttpsPortZero,
    /// `mail.port == 0` (a bindable loopback port must be non-zero).
    MailPortZero,
    /// `dumps.port == 0` (a bindable loopback port must be non-zero).
    DumpsPortZero,
    /// `[services]` contained an instance whose id is not in `KNOWN_SERVICES`.
    UnknownService,
    /// `parked.paths` contained an empty string.
    ParkedPathEmpty,
    /// `overrides` contained an empty-string key.
    OverridePathEmpty,
    /// `php.settings` contained an unsupported key or an invalid value.
    InvalidPhpSetting,
    /// A linked `web_subpath` or override `web_root` was not a plain relative
    /// path (absolute, or contained a `..`/root/prefix component) and could
    /// escape the document root.
    WebRootEscapes,
    /// `update_channel` was not one of the accepted values (`"stable"` / `"edge"`).
    InvalidUpdateChannel,
    /// A `ports.fallback_*` value is below the first unprivileged port (1024).
    /// The rootless fallback must not need elevation, so 80/443 is rejected.
    FallbackPortPrivileged,
    /// `ports.fallback_http == ports.fallback_https`.
    FallbackPortsEqual,
}

impl fmt::Display for ValidateErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::DuplicateLinkedSite => "two linked sites share a name",
            Self::HttpHttpsPortsEqual => "ports.http and ports.https must differ",
            Self::HttpPortZero => "ports.http must be non-zero",
            Self::HttpsPortZero => "ports.https must be non-zero",
            Self::MailPortZero => "mail.port must be non-zero",
            Self::DumpsPortZero => "dumps.port must be non-zero",
            Self::UnknownService => "services contains an unrecognised service id",
            Self::ParkedPathEmpty => "parked.paths contains an empty string",
            Self::OverridePathEmpty => "overrides contains an empty path key",
            Self::InvalidPhpSetting => "php.settings contains an unsupported key or invalid value",
            Self::WebRootEscapes => {
                "a web root must be a plain relative path (no leading '/' or '..')"
            }
            Self::InvalidUpdateChannel => "update_channel must be \"stable\" or \"edge\"",
            Self::FallbackPortPrivileged => {
                "ports.fallback_http and ports.fallback_https must be 1024 or higher"
            }
            Self::FallbackPortsEqual => "ports.fallback_http and ports.fallback_https must differ",
        };
        f.write_str(msg)
    }
}

/// Specific failure modes for the migration pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MigrationErrorReason {
    /// File has no top-level `version` key (or root is not a table at all).
    MissingVersion,
    /// `version` field is present but is not a non-negative integer fitting
    /// in `u32`.
    NonIntegerVersion,
    /// A forward migration step is required to bridge `from` → `from + 1`
    /// but is absent from `migrate::STEPS`. Indicates a STEPS
    /// misconfiguration (developer error), not a user-input error.
    MissingStep {
        /// The version we were trying to migrate up from.
        from: u32,
    },
}

impl fmt::Display for MigrationErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVersion => f.write_str("missing top-level `version` key"),
            Self::NonIntegerVersion => f.write_str("`version` must be a non-negative integer"),
            Self::MissingStep { from } => {
                write!(f, "no migration step registered for version {from}")
            }
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
    use std::io::ErrorKind;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn display_validate_each_variant_non_empty() {
        for r in [
            ValidateErrorReason::DuplicateLinkedSite,
            ValidateErrorReason::HttpHttpsPortsEqual,
            ValidateErrorReason::HttpPortZero,
            ValidateErrorReason::HttpsPortZero,
            ValidateErrorReason::MailPortZero,
            ValidateErrorReason::DumpsPortZero,
            ValidateErrorReason::UnknownService,
            ValidateErrorReason::ParkedPathEmpty,
            ValidateErrorReason::OverridePathEmpty,
            ValidateErrorReason::InvalidPhpSetting,
            ValidateErrorReason::WebRootEscapes,
            ValidateErrorReason::InvalidUpdateChannel,
            ValidateErrorReason::FallbackPortPrivileged,
            ValidateErrorReason::FallbackPortsEqual,
        ] {
            assert!(!r.to_string().is_empty());
            let _ = format!("{r:?}");
        }
    }

    #[test]
    fn display_migration_each_variant_non_empty() {
        for r in [
            MigrationErrorReason::MissingVersion,
            MigrationErrorReason::NonIntegerVersion,
            MigrationErrorReason::MissingStep { from: 0 },
        ] {
            assert!(!r.to_string().is_empty());
            let _ = format!("{r:?}");
        }
    }

    #[test]
    fn display_config_error_parse_carries_input() {
        let err: toml::de::Error = toml::from_str::<toml::Value>("not = valid = toml").unwrap_err();
        let e = ConfigError::Parse(err);
        let s = e.to_string();
        assert!(s.contains("parse"), "missing 'parse' in {s}");
    }

    #[test]
    fn display_config_error_validate_includes_reason() {
        let e = ConfigError::Validate {
            reason: ValidateErrorReason::HttpPortZero,
        };
        let s = e.to_string();
        assert!(s.contains("non-zero"), "missing reason in {s}");
    }

    #[test]
    fn display_config_error_core_wraps_inner() {
        let core = yerd_core::Tld::new("").unwrap_err();
        let e = ConfigError::Core(core);
        let s = e.to_string();
        assert!(s.contains("invalid domain value"), "missing wrapper in {s}");
    }

    #[test]
    fn display_config_error_unsupported_version() {
        let e = ConfigError::UnsupportedVersion {
            found: 99,
            current: 3,
        };
        let s = e.to_string();
        assert!(s.contains("99"), "missing found in {s}");
        assert!(s.contains('3'), "missing current in {s}");
    }

    #[test]
    fn display_config_error_migration_includes_reason() {
        let e = ConfigError::Migration {
            reason: MigrationErrorReason::MissingStep { from: 0 },
        };
        let s = e.to_string();
        assert!(s.contains('0'), "missing from in {s}");
    }

    #[test]
    fn display_config_error_io_includes_path_and_source() {
        let e = ConfigError::Io {
            path: PathBuf::from("/tmp/yerd-test.toml"),
            source: std::io::Error::from(ErrorKind::NotFound),
        };
        let s = e.to_string();
        assert!(s.contains("/tmp/yerd-test.toml"), "missing path in {s}");
    }

    /// Constructs every `ConfigError` variant once. Acts as a tripwire: when
    /// a new variant is added without updating this test, coverage drops.
    #[test]
    fn construct_every_config_error_variant() {
        let _ = ConfigError::Parse(toml::from_str::<toml::Value>("x =").unwrap_err());
        let _ = ConfigError::Serialize(toml_ser_error());
        let _ = ConfigError::Validate {
            reason: ValidateErrorReason::HttpPortZero,
        };
        let _ = ConfigError::Core(yerd_core::Tld::new("").unwrap_err());
        let _ = ConfigError::UnsupportedVersion {
            found: 99,
            current: 3,
        };
        let _ = ConfigError::Migration {
            reason: MigrationErrorReason::MissingVersion,
        };
        let _ = ConfigError::Io {
            path: PathBuf::from("/x"),
            source: std::io::Error::from(ErrorKind::PermissionDenied),
        };
    }

    /// Construct a real `toml::ser::Error` by attempting to serialise a
    /// shape `toml` rejects (an integer key in a map). This exercises the
    /// `Serialize` variant via a genuine failure rather than synthesising it.
    fn toml_ser_error() -> toml::ser::Error {
        use std::collections::BTreeMap;
        let mut top = BTreeMap::new();
        top.insert("k".to_string(), 1i64);
        match toml::to_string(&42i64) {
            Ok(_) => panic!("expected toml::ser to reject non-table root"),
            Err(e) => e,
        }
    }
}
