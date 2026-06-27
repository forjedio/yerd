//! Error types for `yerd-core`.
//!
//! `CoreError` is the single error type exposed by every fallible public API
//! in this crate. Each variant carries a typed `*Reason` sub-enum so callers
//! can match on precise failure modes without parsing message strings.
//!
//! Every public error enum carries `#[non_exhaustive]` so additions are
//! semver-compatible.

use std::fmt;

use thiserror::Error;

/// Errors produced by `yerd-core`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    /// A string failed to parse as a [`PhpVersion`](crate::PhpVersion).
    #[error("invalid PHP version {input:?}: {reason}")]
    InvalidPhpVersion {
        /// The raw input that failed to parse.
        input: String,
        /// Why it failed.
        reason: PhpVersionErrorReason,
    },

    /// A string failed to validate as a [`Tld`](crate::Tld).
    #[error("invalid TLD {input:?}: {reason}")]
    InvalidTld {
        /// The raw input that failed validation.
        input: String,
        /// Why it failed.
        reason: TldErrorReason,
    },

    /// A site with this name is already present in the router.
    #[error("site {name:?} already exists in router")]
    DuplicateSite {
        /// The (lowercased) site name that collided.
        name: String,
    },

    /// No site with this name is present in the router.
    #[error("site {name:?} not found in router")]
    SiteNotFound {
        /// The (lowercased) site name that was looked up.
        name: String,
    },

    /// A site name failed validation.
    #[error("site name {name:?} is invalid: {reason}")]
    InvalidSiteName {
        /// The raw name that failed validation.
        name: String,
        /// Why it failed.
        reason: SiteNameErrorReason,
    },
}

/// Specific failure modes for [`PhpVersion`](crate::PhpVersion) parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhpVersionErrorReason {
    /// Input was empty.
    Empty,
    /// Input parsed a major number but had no `.minor` part.
    MissingMinor,
    /// Major or minor contained non-digit bytes, was empty, or overflowed `u16`.
    NonNumeric,
    /// Input had a non-empty prefix that wasn't ASCII "php" (case-insensitive),
    /// or anything followed `php` other than digits.
    UnsupportedPrefix,
    /// Major parsed but fell outside the accepted `5..=9` range.
    MajorOutOfRange,
    /// Minor parsed but fell outside the accepted `0..=99` range.
    MinorOutOfRange,
}

impl fmt::Display for PhpVersionErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "input is empty",
            Self::MissingMinor => "missing minor version",
            Self::NonNumeric => "version parts must be ASCII digits",
            Self::UnsupportedPrefix => "only the ASCII \"php\" prefix is accepted",
            Self::MajorOutOfRange => "major version must be 5..=9",
            Self::MinorOutOfRange => "minor version must be 0..=99",
        };
        f.write_str(msg)
    }
}

/// Specific failure modes for [`Tld`](crate::Tld) construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TldErrorReason {
    /// Input was empty (possibly after stripping one trailing dot).
    Empty,
    /// Input started with `.`, or still ended with `.` after the single trailing-dot strip.
    LeadingOrTrailingDot,
    /// Two adjacent dots produced an empty label.
    ConsecutiveDots,
    /// Input contained ASCII whitespace.
    ContainsWhitespace,
    /// Input contained a byte > 0x7F (non-ASCII).
    NonAscii,
    /// A label contained a character not in `[a-z0-9-]`.
    InvalidCharacter,
    /// A label exceeded 63 bytes.
    LabelTooLong,
    /// A label started or ended with `-`.
    LeadingOrTrailingHyphen,
    /// Total input length exceeded 253 bytes (RFC 1035).
    TooLong,
}

impl fmt::Display for TldErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "tld must not be empty",
            Self::LeadingOrTrailingDot => "tld must not have a leading or trailing dot",
            Self::ConsecutiveDots => "tld must not contain consecutive dots",
            Self::ContainsWhitespace => "tld must not contain whitespace",
            Self::NonAscii => "tld must be ASCII",
            Self::InvalidCharacter => "tld labels may only contain [a-z0-9-]",
            Self::LabelTooLong => "tld label exceeds 63 bytes",
            Self::LeadingOrTrailingHyphen => "tld labels must not start or end with '-'",
            Self::TooLong => "tld exceeds 253 bytes",
        };
        f.write_str(msg)
    }
}

/// Specific failure modes for site-name validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SiteNameErrorReason {
    /// Input was empty.
    Empty,
    /// Site names are single DNS labels and must not contain `.`.
    ContainsDot,
    /// A character was outside the DNS-safe alphabet `[a-z0-9-]`,
    /// or input was non-ASCII / contained whitespace.
    InvalidCharacter,
    /// Site name exceeded 63 bytes (RFC 1035 single label).
    LabelTooLong,
    /// Site name started or ended with `-`.
    LeadingOrTrailingHyphen,
}

impl fmt::Display for SiteNameErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "site name must not be empty",
            Self::ContainsDot => "site name must not contain '.'",
            Self::InvalidCharacter => "site name may only contain [a-z0-9-]",
            Self::LabelTooLong => "site name exceeds 63 bytes",
            Self::LeadingOrTrailingHyphen => "site name must not start or end with '-'",
        };
        f.write_str(msg)
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
    fn display_invalid_php_contains_input_and_reason() {
        let e = CoreError::InvalidPhpVersion {
            input: "v8.3".into(),
            reason: PhpVersionErrorReason::UnsupportedPrefix,
        };
        let s = e.to_string();
        assert!(s.contains("v8.3"), "missing input in: {s}");
        assert!(s.contains("php"), "missing reason phrase in: {s}");
    }

    #[test]
    fn display_duplicate_site_contains_name() {
        let e = CoreError::DuplicateSite { name: "foo".into() };
        assert!(e.to_string().contains("foo"));
    }

    #[test]
    fn display_site_not_found_contains_name() {
        let e = CoreError::SiteNotFound { name: "bar".into() };
        assert!(e.to_string().contains("bar"));
    }

    #[test]
    fn error_is_send_sync_clone_eq() {
        fn assert_traits<T: Send + Sync + Clone + Eq>() {}
        assert_traits::<CoreError>();
        assert_traits::<PhpVersionErrorReason>();
        assert_traits::<TldErrorReason>();
        assert_traits::<SiteNameErrorReason>();
    }

    /// Construct every variant of `CoreError` and every `*Reason` variant.
    /// Acts as a tripwire: when a new variant is added without updating this
    /// test, coverage drops and the omission becomes visible.
    #[test]
    fn construct_every_error_variant() {
        let _ = CoreError::InvalidPhpVersion {
            input: "x".into(),
            reason: PhpVersionErrorReason::Empty,
        };
        let _ = CoreError::InvalidTld {
            input: "x".into(),
            reason: TldErrorReason::Empty,
        };
        let _ = CoreError::DuplicateSite { name: "x".into() };
        let _ = CoreError::SiteNotFound { name: "x".into() };
        let _ = CoreError::InvalidSiteName {
            name: "x".into(),
            reason: SiteNameErrorReason::Empty,
        };

        for r in [
            PhpVersionErrorReason::Empty,
            PhpVersionErrorReason::MissingMinor,
            PhpVersionErrorReason::NonNumeric,
            PhpVersionErrorReason::UnsupportedPrefix,
            PhpVersionErrorReason::MajorOutOfRange,
            PhpVersionErrorReason::MinorOutOfRange,
        ] {
            assert!(!r.to_string().is_empty());
            let _debug = format!("{r:?}");
        }

        for r in [
            TldErrorReason::Empty,
            TldErrorReason::LeadingOrTrailingDot,
            TldErrorReason::ConsecutiveDots,
            TldErrorReason::ContainsWhitespace,
            TldErrorReason::NonAscii,
            TldErrorReason::InvalidCharacter,
            TldErrorReason::LabelTooLong,
            TldErrorReason::LeadingOrTrailingHyphen,
            TldErrorReason::TooLong,
        ] {
            assert!(!r.to_string().is_empty());
            let _debug = format!("{r:?}");
        }

        for r in [
            SiteNameErrorReason::Empty,
            SiteNameErrorReason::ContainsDot,
            SiteNameErrorReason::InvalidCharacter,
            SiteNameErrorReason::LabelTooLong,
            SiteNameErrorReason::LeadingOrTrailingHyphen,
        ] {
            assert!(!r.to_string().is_empty());
            let _debug = format!("{r:?}");
        }
    }
}
