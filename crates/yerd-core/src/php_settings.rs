//! The global PHP ini settings Yerd manages, with pure validation.
//!
//! Yerd lets users set a small, fixed set of PHP runtime directives
//! (`memory_limit`, `upload_max_filesize`, …) that are written into **every**
//! installed version's FPM pool config as `php_value[...]` / `php_flag[...]`
//! lines. The values flow, unescaped, straight into the FPM master config
//! file, so [`validate_value`] is the **security boundary**: it is run when a
//! value is set (CLI + daemon), when the config is loaded from disk
//! (`yerd-config`), and again defensively at render time (`yerd-php`).
//!
//! This module is pure: an allowlist of supported directives plus per-kind
//! value validators, hand-rolled (no `regex` dependency).

use std::fmt;

use thiserror::Error;

/// Longest accepted value, in bytes. Generous for `error_reporting` constant
/// expressions while bounding the FPM config line.
const MAX_VALUE_LEN: usize = 256;

/// How a supported setting's value is validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A byte size: digits with an optional `K`/`M`/`G` suffix (any case).
    /// `allow_unlimited` additionally accepts the literal `-1`.
    Bytes { allow_unlimited: bool },
    /// A non-negative integer (`0` is allowed).
    Int,
    /// A boolean, rendered as a `php_flag` (`On`/`Off`).
    Flag,
    /// An `error_reporting` bitmask: an integer or a constant expression such
    /// as `E_ALL & ~E_DEPRECATED`.
    ErrorReporting,
}

/// One supported setting and how to validate it.
struct Spec {
    name: &'static str,
    kind: Kind,
}

/// The fixed allowlist. Extend here to support more directives.
const SETTINGS: &[Spec] = &[
    Spec {
        name: "memory_limit",
        kind: Kind::Bytes {
            allow_unlimited: true,
        },
    },
    Spec {
        name: "max_execution_time",
        kind: Kind::Int,
    },
    Spec {
        name: "max_input_time",
        kind: Kind::Int,
    },
    Spec {
        name: "max_file_uploads",
        kind: Kind::Int,
    },
    Spec {
        name: "upload_max_filesize",
        kind: Kind::Bytes {
            allow_unlimited: false,
        },
    },
    Spec {
        name: "post_max_size",
        kind: Kind::Bytes {
            allow_unlimited: false,
        },
    },
    Spec {
        name: "display_errors",
        kind: Kind::Flag,
    },
    Spec {
        name: "error_reporting",
        kind: Kind::ErrorReporting,
    },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SETTINGS.iter().find(|s| s.name == name)
}

/// Whether `name` is a setting Yerd manages.
#[must_use]
pub fn is_supported(name: &str) -> bool {
    spec(name).is_some()
}

/// The supported setting names, in declaration order (for CLI help / errors).
#[must_use]
pub fn supported_names() -> Vec<&'static str> {
    SETTINGS.iter().map(|s| s.name).collect()
}

/// The FPM directive a setting renders as: `"php_flag"` for booleans, else
/// `"php_value"`. `None` if the setting is not supported.
#[must_use]
pub fn directive(name: &str) -> Option<&'static str> {
    spec(name).map(|s| match s.kind {
        Kind::Flag => "php_flag",
        _ => "php_value",
    })
}

/// Validate `value` for the supported setting `name`.
///
/// Enforces a global safety invariant (non-empty, not all-whitespace,
/// `≤ MAX_VALUE_LEN`, no control characters, none of the FPM/ini
/// metacharacters `[ ] = ; #`) plus a per-kind shape. This is the security
/// boundary protecting the rendered FPM config from injection.
///
/// # Errors
/// [`PhpSettingError::Unsupported`] for an unknown `name`;
/// [`PhpSettingError::InvalidValue`] for a value failing the invariant or shape.
pub fn validate_value(name: &str, value: &str) -> Result<(), PhpSettingError> {
    let spec = spec(name).ok_or_else(|| PhpSettingError::Unsupported {
        name: name.to_owned(),
    })?;
    let err = |reason| PhpSettingError::InvalidValue {
        name: name.to_owned(),
        reason,
    };

    // Global invariant.
    if value.is_empty() || value.chars().all(char::is_whitespace) {
        return Err(err(ValueErrorReason::Empty));
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(err(ValueErrorReason::TooLong));
    }
    for c in value.chars() {
        if c.is_control() || matches!(c, '[' | ']' | '=' | ';' | '#') {
            return Err(err(ValueErrorReason::IllegalCharacter));
        }
    }

    // Per-kind shape, on the trimmed value.
    let t = value.trim();
    let ok = match spec.kind {
        Kind::Bytes { allow_unlimited } => is_byte_size(t, allow_unlimited),
        Kind::Int => is_uint(t),
        Kind::Flag => parse_flag(t).is_some(),
        Kind::ErrorReporting => is_error_reporting(t),
    };
    if ok {
        Ok(())
    } else {
        Err(err(match spec.kind {
            Kind::Bytes { .. } => ValueErrorReason::NotAByteSize,
            Kind::Int => ValueErrorReason::NotAnInteger,
            Kind::Flag => ValueErrorReason::NotABoolean,
            Kind::ErrorReporting => ValueErrorReason::NotErrorReporting,
        }))
    }
}

/// Normalise a (validated) value to its canonical stored/rendered form:
/// booleans become `On`/`Off`; everything else is trimmed. Unknown settings
/// are returned trimmed unchanged.
#[must_use]
pub fn canonical_value(name: &str, value: &str) -> String {
    match spec(name).map(|s| s.kind) {
        Some(Kind::Flag) => match parse_flag(value.trim()) {
            Some(true) => "On".to_owned(),
            Some(false) => "Off".to_owned(),
            None => value.trim().to_owned(),
        },
        _ => value.trim().to_owned(),
    }
}

/// `\d+` with an optional single `K`/`M`/`G` suffix (any case); plus the
/// literal `-1` when `allow_unlimited`.
fn is_byte_size(s: &str, allow_unlimited: bool) -> bool {
    if allow_unlimited && s == "-1" {
        return true;
    }
    let digits = match s.strip_suffix(['K', 'M', 'G', 'k', 'm', 'g']) {
        Some(rest) => rest,
        None => s,
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// One-or-more ASCII digits.
fn is_uint(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Case-insensitive boolean: `on|off|1|0|true|false`.
fn parse_flag(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// An integer or a constant expression: characters limited to
/// `[A-Za-z0-9_ &|~^()-]`, with at least one non-space character.
fn is_error_reporting(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace())
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | ' ' | '&' | '|' | '~' | '^' | '(' | ')' | '-')
        })
}

/// Failure to set or validate a managed PHP setting.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhpSettingError {
    /// `name` is not a setting Yerd manages.
    #[error("unknown PHP setting {name:?}")]
    Unsupported {
        /// The rejected setting name.
        name: String,
    },
    /// The value failed the safety invariant or the setting's shape.
    #[error("invalid value for PHP setting {name:?}: {reason}")]
    InvalidValue {
        /// The setting whose value was rejected.
        name: String,
        /// Why the value was rejected.
        reason: ValueErrorReason,
    },
}

/// Specific failure modes for a PHP setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueErrorReason {
    /// Empty or all-whitespace.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// Contained a control char or an FPM/ini metacharacter (`[ ] = ; #`).
    IllegalCharacter,
    /// Not a byte size (`\d+[KMG]?`, or `-1` for `memory_limit`).
    NotAByteSize,
    /// Not a non-negative integer.
    NotAnInteger,
    /// Not a recognised boolean (`on|off|1|0|true|false`).
    NotABoolean,
    /// Not a valid `error_reporting` integer or constant expression.
    NotErrorReporting,
}

impl fmt::Display for ValueErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "value must not be empty",
            Self::TooLong => "value is too long",
            Self::IllegalCharacter => "value contains a control or reserved character ([ ] = ; #)",
            Self::NotAByteSize => "expected a byte size like 256M, 1G, or -1",
            Self::NotAnInteger => "expected a non-negative integer",
            Self::NotABoolean => "expected On or Off",
            Self::NotErrorReporting => {
                "expected an integer or a constant like E_ALL & ~E_DEPRECATED"
            }
        };
        f.write_str(msg)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn supported_set_and_directives() {
        assert!(is_supported("memory_limit"));
        assert!(!is_supported("allow_url_fopen"));
        assert_eq!(directive("memory_limit"), Some("php_value"));
        assert_eq!(directive("display_errors"), Some("php_flag"));
        assert_eq!(directive("nope"), None);
        assert_eq!(supported_names().len(), 8);
    }

    #[test]
    fn unsupported_name_is_rejected() {
        assert!(matches!(
            validate_value("allow_url_fopen", "1"),
            Err(PhpSettingError::Unsupported { .. })
        ));
    }

    #[test]
    fn byte_sizes() {
        for v in ["256M", "1G", "512m", "1024", "100K", "-1"] {
            assert!(validate_value("memory_limit", v).is_ok(), "{v}");
        }
        for v in ["256MB", "1.5G", "M", "12X", "-5", "-1K"] {
            assert!(validate_value("memory_limit", v).is_err(), "{v}");
        }
        // -1 (unlimited) only for memory_limit.
        assert!(validate_value("upload_max_filesize", "-1").is_err());
        assert!(validate_value("upload_max_filesize", "100M").is_ok());
    }

    #[test]
    fn integers() {
        assert!(validate_value("max_execution_time", "0").is_ok());
        assert!(validate_value("max_execution_time", "300").is_ok());
        assert!(validate_value("max_file_uploads", "20").is_ok());
        assert!(validate_value("max_execution_time", "30s").is_err());
        assert!(validate_value("max_execution_time", "-1").is_err());
    }

    #[test]
    fn flags_validate_and_canonicalise() {
        for v in ["On", "off", "1", "0", "true", "FALSE"] {
            assert!(validate_value("display_errors", v).is_ok(), "{v}");
        }
        assert!(validate_value("display_errors", "yes").is_err());
        assert_eq!(canonical_value("display_errors", "on"), "On");
        assert_eq!(canonical_value("display_errors", "0"), "Off");
        assert_eq!(canonical_value("memory_limit", "  512M "), "512M");
    }

    #[test]
    fn error_reporting_accepts_constants_and_ints() {
        for v in [
            "E_ALL",
            "E_ALL & ~E_DEPRECATED & ~E_NOTICE",
            "22519",
            "E_ALL | E_STRICT",
        ] {
            assert!(validate_value("error_reporting", v).is_ok(), "{v}");
        }
        assert!(validate_value("error_reporting", "   ").is_err());
    }

    #[test]
    fn injection_attempts_are_rejected() {
        // Newline, FPM/ini metacharacters, and over-length are all blocked.
        assert!(validate_value("memory_limit", "1\nuser = root").is_err());
        assert!(validate_value("memory_limit", "256M; evil").is_err());
        assert!(validate_value("error_reporting", "E_ALL # comment").is_err());
        assert!(validate_value("error_reporting", "E_ALL]").is_err());
        assert!(validate_value("memory_limit", "256M=x").is_err());
        assert!(validate_value("memory_limit", &"9".repeat(MAX_VALUE_LEN + 1)).is_err());
        assert!(validate_value("memory_limit", "   ").is_err());
        assert!(validate_value("memory_limit", "").is_err());
    }
}
