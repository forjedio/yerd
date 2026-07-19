//! Pure validation for free-form per-version PHP ini directives.
//!
//! Beyond the fixed allowlist in [`crate::php_settings`], Yerd lets users set
//! arbitrary ini directives per installed PHP version (`xdebug.mode`,
//! `opcache.enable`, …). These are not typed: Yerd cannot know the shape of
//! every extension's directives, so a valid-shaped but nonsensical entry is
//! PHP's problem, not Yerd's. What this module does guarantee is that a
//! directive can never corrupt a generated FPM pool config or CLI ini:
//! [`validate_name`] and [`validate_value`] are the **injection boundary**,
//! run when a directive is set (CLI + daemon), when the config is loaded from
//! disk (`yerd-config`, leniently: bad entries are dropped), and again
//! defensively at render time (`yerd-php`, `bin/yerdd`).
//!
//! A small denylist ([`reserved`]) keeps free-form entries from colliding with
//! directives Yerd manages through typed paths (the settings allowlist,
//! `yerd php ext`, the CA bundle).
//!
//! This module is pure: string validation and rendering only, hand-rolled
//! (no `regex` dependency).

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

use crate::php_settings;

/// Longest accepted directive name, in bytes. Real ini directive names are
/// short (`opcache.jit_buffer_size` is 23 bytes); 128 is a generous bound.
const MAX_NAME_LEN: usize = 128;

/// Longest accepted directive value, in bytes. Matches the allowlisted
/// settings' cap in [`crate::php_settings`].
const MAX_VALUE_LEN: usize = 256;

/// Denylisted directive names Yerd manages through a typed path, paired with a
/// human-readable hint pointing at that path.
const RESERVED: &[(&str, &str)] = &[
    (
        "extension",
        "extensions are managed with `yerd php ext` (or the GUI extensions panel)",
    ),
    (
        "zend_extension",
        "extensions are managed with `yerd php ext` (or the GUI extensions panel)",
    ),
    ("openssl.cafile", "Yerd manages the CA bundle for this"),
    ("curl.cainfo", "Yerd manages the CA bundle for this"),
];

/// If `name` is a directive Yerd manages elsewhere, the human-readable hint
/// explaining where; `None` when the name is free for custom use.
///
/// Covers the typed settings allowlist (use `yerd set php <name>` /
/// `yerd unset php <name>`, optionally with `--only <version>`), extension
/// loading, and the CA bundle paths.
#[must_use]
pub fn reserved(name: &str) -> Option<&'static str> {
    if php_settings::is_supported(name) {
        return Some("this setting is managed with `yerd set php` (add --only <version> for a per-version value)");
    }
    RESERVED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, hint)| *hint)
}

/// Validate a free-form directive name: non-empty, bounded, first character
/// `[A-Za-z_]`, remaining characters `[A-Za-z0-9._-]`. This is strict enough
/// to keep a name safe on the left side of an unescaped `php_value[name]` /
/// `name =` line while accepting every real ini directive shape
/// (`xdebug.mode`, `opcache.jit_buffer_size`, `zend.assertions`).
///
/// Reserved names ([`reserved`]) are not rejected here; callers on the set
/// path check that separately so they can surface the specific hint.
///
/// # Errors
/// [`DirectiveError::Name`] with the specific [`DirectiveNameErrorReason`].
pub fn validate_name(name: &str) -> Result<(), DirectiveError> {
    let err = |reason| Err(DirectiveError::Name { reason });
    let Some(first) = name.chars().next() else {
        return err(DirectiveNameErrorReason::Empty);
    };
    if name.len() > MAX_NAME_LEN {
        return err(DirectiveNameErrorReason::TooLong);
    }
    if !(first.is_ascii_alphabetic() || first == '_') {
        return err(DirectiveNameErrorReason::IllegalStart);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return err(DirectiveNameErrorReason::IllegalCharacter);
    }
    Ok(())
}

/// Validate a free-form directive value: non-empty, not all-whitespace,
/// `≤ MAX_VALUE_LEN`, no control characters, none of the FPM/ini
/// metacharacters `[ ] = ; #`. This is the same charset discipline as
/// [`php_settings::validate_value`] without the per-kind typing: it keeps the
/// value from breaking out of its config line, and leaves semantic sanity to
/// PHP.
///
/// # Errors
/// [`DirectiveError::Value`] with the specific
/// [`php_settings::ValueErrorReason`].
pub fn validate_value(value: &str) -> Result<(), DirectiveError> {
    use php_settings::ValueErrorReason;
    let err = |reason| Err(DirectiveError::Value { reason });
    if value.is_empty() || value.chars().all(char::is_whitespace) {
        return err(ValueErrorReason::Empty);
    }
    if value.len() > MAX_VALUE_LEN {
        return err(ValueErrorReason::TooLong);
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '[' | ']' | '=' | ';' | '#'))
    {
        return err(ValueErrorReason::IllegalCharacter);
    }
    Ok(())
}

/// Render a directives map as `name = value` ini lines for a CLI `php.ini`,
/// in map (alphabetical) order. Entries failing [`validate_name`] /
/// [`validate_value`] or naming a [`reserved`] directive are skipped
/// defensively - this renderer runs after the set-time and load-time checks,
/// so nothing malformed can reach a generated file even if a bad entry slips
/// through. Values are trimmed; empty when nothing renders.
#[must_use]
pub fn render_ini_lines(directives: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (name, value) in directives {
        if validate_name(name).is_err()
            || validate_value(value).is_err()
            || reserved(name).is_some()
        {
            continue;
        }
        out.push_str(name);
        out.push_str(" = ");
        out.push_str(value.trim());
        out.push('\n');
    }
    out
}

/// Failure to validate a free-form ini directive.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DirectiveError {
    /// The directive name was rejected.
    #[error("invalid ini directive name: {reason}")]
    Name {
        /// Why the name was rejected.
        reason: DirectiveNameErrorReason,
    },
    /// The directive value was rejected.
    #[error("invalid ini directive value: {reason}")]
    Value {
        /// Why the value was rejected.
        reason: php_settings::ValueErrorReason,
    },
}

/// Specific failure modes for a directive name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DirectiveNameErrorReason {
    /// Empty string.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// First character was not a letter or underscore.
    IllegalStart,
    /// Contained a character outside `[A-Za-z0-9._-]`.
    IllegalCharacter,
}

impl fmt::Display for DirectiveNameErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "name must not be empty",
            Self::TooLong => "name is too long",
            Self::IllegalStart => "name must start with a letter or '_'",
            Self::IllegalCharacter => "name may only contain letters, digits, '.', '_' and '-'",
        };
        f.write_str(msg)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_pass() {
        for name in [
            "xdebug.mode",
            "opcache.enable",
            "opcache.jit_buffer_size",
            "zend.assertions",
            "_private",
            "short_open_tag",
            "a",
        ] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn invalid_names_are_rejected() {
        let cases: &[(&str, DirectiveNameErrorReason)] = &[
            ("", DirectiveNameErrorReason::Empty),
            ("1st", DirectiveNameErrorReason::IllegalStart),
            (".dot", DirectiveNameErrorReason::IllegalStart),
            ("-dash", DirectiveNameErrorReason::IllegalStart),
            ("has space", DirectiveNameErrorReason::IllegalCharacter),
            ("semi;colon", DirectiveNameErrorReason::IllegalCharacter),
            ("brack[et", DirectiveNameErrorReason::IllegalCharacter),
            ("eq=uals", DirectiveNameErrorReason::IllegalCharacter),
            ("new\nline", DirectiveNameErrorReason::IllegalCharacter),
        ];
        for (name, want) in cases {
            assert!(
                matches!(validate_name(name), Err(DirectiveError::Name { reason }) if reason == *want),
                "{name:?}"
            );
        }
        assert!(matches!(
            validate_name(&"x".repeat(MAX_NAME_LEN + 1)),
            Err(DirectiveError::Name {
                reason: DirectiveNameErrorReason::TooLong
            })
        ));
    }

    #[test]
    fn valid_values_pass() {
        for value in ["debug", "1", "off", "develop,debug", "256M", "/a/b c.log"] {
            assert!(validate_value(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn invalid_values_are_rejected() {
        use crate::php_settings::ValueErrorReason;
        let cases: &[(&str, ValueErrorReason)] = &[
            ("", ValueErrorReason::Empty),
            ("   ", ValueErrorReason::Empty),
            ("a\nb", ValueErrorReason::IllegalCharacter),
            ("a;b", ValueErrorReason::IllegalCharacter),
            ("a#b", ValueErrorReason::IllegalCharacter),
            ("a=b", ValueErrorReason::IllegalCharacter),
            ("a[b", ValueErrorReason::IllegalCharacter),
            ("a]b", ValueErrorReason::IllegalCharacter),
        ];
        for (value, want) in cases {
            assert!(
                matches!(validate_value(value), Err(DirectiveError::Value { reason }) if reason == *want),
                "{value:?}"
            );
        }
        assert!(matches!(
            validate_value(&"9".repeat(MAX_VALUE_LEN + 1)),
            Err(DirectiveError::Value {
                reason: ValueErrorReason::TooLong
            })
        ));
    }

    #[test]
    fn every_reserved_name_has_a_hint() {
        for name in [
            "extension",
            "zend_extension",
            "openssl.cafile",
            "curl.cainfo",
            "memory_limit",
            "max_execution_time",
            "max_input_time",
            "max_file_uploads",
            "upload_max_filesize",
            "post_max_size",
            "display_errors",
            "error_reporting",
        ] {
            assert!(reserved(name).is_some(), "{name}");
        }
        assert!(reserved("xdebug.mode").is_none());
        assert!(reserved("opcache.enable").is_none());
    }

    #[test]
    fn render_skips_invalid_and_reserved_entries() {
        let directives = BTreeMap::from([
            ("xdebug.mode".to_owned(), "debug".to_owned()),
            ("opcache.enable".to_owned(), " 1 ".to_owned()),
            ("bad name".to_owned(), "x".to_owned()),
            ("bad.value".to_owned(), "a;b".to_owned()),
            ("extension".to_owned(), "/evil.so".to_owned()),
            ("memory_limit".to_owned(), "1G".to_owned()),
        ]);
        assert_eq!(
            render_ini_lines(&directives),
            "opcache.enable = 1\nxdebug.mode = debug\n"
        );
        assert_eq!(render_ini_lines(&BTreeMap::new()), "");
    }

    #[test]
    fn error_display_non_empty() {
        for r in [
            DirectiveNameErrorReason::Empty,
            DirectiveNameErrorReason::TooLong,
            DirectiveNameErrorReason::IllegalStart,
            DirectiveNameErrorReason::IllegalCharacter,
        ] {
            assert!(!r.to_string().is_empty());
        }
    }
}
