//! Pure validation for per-version FPM pool settings.
//!
//! FPM's pool-level knobs (`pm`, `pm.max_children`, `pm.start_servers`, …) are
//! not ini directives: they sit in the pool block of the generated config, not
//! behind `php_value[…]`. Setting one through the free-form directives path
//! renders `php_value[pm.max_children]`, which FPM refuses with
//! `ERROR: Unable to set php_value 'pm.max_children'` on every worker spawn -
//! so [`crate::php_directives::reserved`] denies the whole `pm.` prefix and
//! points here instead.
//!
//! Yerd exposes exactly one of these knobs: `max_children`, the ceiling on
//! concurrent PHP workers for a version's pool. The pool runs `ondemand`, so
//! raising the ceiling costs nothing while idle - workers are spawned on
//! demand rather than preallocated.
//!
//! This module is pure: string validation and a hand-rolled integer parse only
//! (no `regex` dependency), mirroring [`crate::php_directives`].

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

/// The pool's worker ceiling when a version has no override. Single source of
/// truth: `yerd-php`'s `PoolConfig::dev_defaults` renders it and the CLI
/// displays it, so the two can never drift.
pub const DEFAULT_MAX_CHILDREN: u32 = 16;

/// Smallest accepted [`DEFAULT_MAX_CHILDREN`] override. `0` would leave the
/// pool unable to serve a request at all.
const MIN_MAX_CHILDREN: u32 = 1;

/// Largest accepted override. Far above any local development workload; the
/// cap is here to catch a typo (`3200` for `32`), not to police the user.
const MAX_MAX_CHILDREN: u32 = 1024;

/// The only pool setting Yerd exposes.
const MAX_CHILDREN: &str = "max_children";

/// Validate a pool setting name.
///
/// Unlike [`crate::php_directives::validate_name`], this is an allowlist of
/// one: pool settings are a typed surface, not free-form ini.
///
/// # Errors
/// [`PoolSettingError::Name`] when `name` is anything but `max_children`.
pub fn validate_name(name: &str) -> Result<(), PoolSettingError> {
    if name == MAX_CHILDREN {
        return Ok(());
    }
    Err(PoolSettingError::Name {
        reason: PoolNameErrorReason::Unknown,
    })
}

/// Validate a pool setting value and return it parsed.
///
/// Accepts a plain run of ASCII digits (optionally surrounded by whitespace)
/// inside `1..=1024`. A leading `+`/`-`, a decimal point, or any other
/// character is rejected rather than coerced.
///
/// # Errors
/// [`PoolSettingError::Value`] with the specific [`PoolValueErrorReason`].
pub fn validate_value(value: &str) -> Result<u32, PoolSettingError> {
    let err = |reason| Err(PoolSettingError::Value { reason });
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return err(PoolValueErrorReason::Empty);
    }
    if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return err(PoolValueErrorReason::NotANumber);
    }
    let Ok(parsed) = trimmed.parse::<u32>() else {
        return err(PoolValueErrorReason::OutOfRange);
    };
    if !(MIN_MAX_CHILDREN..=MAX_MAX_CHILDREN).contains(&parsed) {
        return err(PoolValueErrorReason::OutOfRange);
    }
    Ok(parsed)
}

/// The `max_children` override in `settings`, when there is a valid one.
///
/// `None` means "leave the built-in default alone": the version has no
/// overrides at all, no `max_children` entry, or an entry that no longer
/// validates. Callers apply the value conditionally, so a bad entry that
/// somehow reached disk degrades to [`DEFAULT_MAX_CHILDREN`] rather than
/// breaking the pool.
#[must_use]
pub fn override_max_children(settings: Option<&BTreeMap<String, String>>) -> Option<u32> {
    settings?
        .get(MAX_CHILDREN)
        .and_then(|v| validate_value(v).ok())
}

/// Failure to validate a pool setting.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolSettingError {
    /// The setting name was rejected.
    #[error("invalid FPM pool setting: {reason}")]
    Name {
        /// Why the name was rejected.
        reason: PoolNameErrorReason,
    },
    /// The setting value was rejected.
    #[error("invalid FPM pool setting value: {reason}")]
    Value {
        /// Why the value was rejected.
        reason: PoolValueErrorReason,
    },
}

/// Specific failure modes for a pool setting name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolNameErrorReason {
    /// Not one of the settings Yerd exposes.
    Unknown,
}

impl fmt::Display for PoolNameErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "the only pool setting is '{MAX_CHILDREN}'"),
        }
    }
}

/// Specific failure modes for a pool setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolValueErrorReason {
    /// Empty or all-whitespace.
    Empty,
    /// Contained something other than ASCII digits.
    NotANumber,
    /// Parsed, but outside the accepted range.
    OutOfRange,
}

impl fmt::Display for PoolValueErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("value must not be empty"),
            Self::NotANumber => f.write_str("value must be a whole number"),
            Self::OutOfRange => {
                write!(
                    f,
                    "value must be between {MIN_MAX_CHILDREN} and {MAX_MAX_CHILDREN}"
                )
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn only_max_children_is_accepted() {
        assert!(validate_name("max_children").is_ok());
        for name in [
            "",
            "max_child",
            "maxchildren",
            "pm.max_children",
            "start_servers",
            "max_requests",
            "Max_Children",
        ] {
            assert!(
                matches!(
                    validate_name(name),
                    Err(PoolSettingError::Name {
                        reason: PoolNameErrorReason::Unknown
                    })
                ),
                "{name:?}"
            );
        }
    }

    #[test]
    fn valid_values_parse() {
        let cases: &[(&str, u32)] = &[
            ("1", 1),
            ("16", 16),
            ("32", 32),
            ("1024", 1024),
            (" 64 ", 64),
            ("032", 32),
        ];
        for (value, want) in cases {
            assert_eq!(validate_value(value), Ok(*want), "{value:?}");
        }
    }

    #[test]
    fn invalid_values_are_rejected() {
        let cases: &[(&str, PoolValueErrorReason)] = &[
            ("", PoolValueErrorReason::Empty),
            ("   ", PoolValueErrorReason::Empty),
            ("abc", PoolValueErrorReason::NotANumber),
            ("-1", PoolValueErrorReason::NotANumber),
            ("+8", PoolValueErrorReason::NotANumber),
            ("16.5", PoolValueErrorReason::NotANumber),
            ("1 6", PoolValueErrorReason::NotANumber),
            ("0x10", PoolValueErrorReason::NotANumber),
            ("0", PoolValueErrorReason::OutOfRange),
            ("1025", PoolValueErrorReason::OutOfRange),
            ("99999999999999999999", PoolValueErrorReason::OutOfRange),
        ];
        for (value, want) in cases {
            assert!(
                matches!(validate_value(value), Err(PoolSettingError::Value { reason }) if reason == *want),
                "{value:?}"
            );
        }
    }

    #[test]
    fn override_is_read_only_when_present_and_valid() {
        assert_eq!(override_max_children(None), None);
        assert_eq!(override_max_children(Some(&BTreeMap::new())), None);

        let valid = BTreeMap::from([("max_children".to_owned(), "32".to_owned())]);
        assert_eq!(override_max_children(Some(&valid)), Some(32));

        for bad in ["0", "1025", "abc", ""] {
            let map = BTreeMap::from([("max_children".to_owned(), bad.to_owned())]);
            assert_eq!(override_max_children(Some(&map)), None, "{bad:?}");
        }

        let other = BTreeMap::from([("start_servers".to_owned(), "4".to_owned())]);
        assert_eq!(override_max_children(Some(&other)), None);
    }

    #[test]
    fn default_is_inside_the_accepted_range() {
        assert_eq!(validate_value(&DEFAULT_MAX_CHILDREN.to_string()), Ok(16));
    }

    #[test]
    fn error_display_non_empty() {
        assert!(!PoolNameErrorReason::Unknown.to_string().is_empty());
        for r in [
            PoolValueErrorReason::Empty,
            PoolValueErrorReason::NotANumber,
            PoolValueErrorReason::OutOfRange,
        ] {
            assert!(!r.to_string().is_empty());
        }
    }
}
