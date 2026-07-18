//! PHP version type.
//!
//! `PhpVersion` is a `(major, minor)` pair with strict acceptance and a
//! human-friendly string form (`"8.3"`, optionally with a case-insensitive
//! `"php"` prefix on parse).

use std::fmt;
use std::str::FromStr;

use crate::error::{CoreError, PhpVersionErrorReason};

/// A PHP major.minor version.
///
/// Display: `"8.3"`. Parse accepts `"8.3"` and `"php8.3"` (case-insensitive on
/// the prefix). Numeric ranges: `major ∈ 5..=9`, `minor ∈ 0..=99`. Larger
/// values that still fit `u16` produce `MajorOutOfRange` / `MinorOutOfRange`;
/// values that overflow `u16` (≥ 65536) produce `NonNumeric`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhpVersion {
    /// Major PHP version (e.g. `8`).
    pub major: u8,
    /// Minor PHP version (e.g. `3`).
    pub minor: u8,
}

/// The lowest PHP minor Yerd considers "supported". The bundled `pcov` and
/// `yerd-dump` extensions are only built for 8.2+, so anything below this is a
/// legacy version: installable, but with no coverage, no dumps, and never
/// eligible as the global default. This is the single authority every legacy
/// guardrail keys on; see [`PhpVersion::is_legacy`].
pub const FIRST_SUPPORTED_MINOR: PhpVersion = PhpVersion::new(8, 2);

impl PhpVersion {
    /// Constructs without validation. Use [`PhpVersion::from_str`] for input
    /// from users or config files.
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    /// True for out-of-support legacy minors (below [`FIRST_SUPPORTED_MINOR`]):
    /// no pcov/coverage, no yerd-dump capture, and rejected as the global
    /// default. The single authority every guardrail keys on (cover shim,
    /// set-default, dumps warning, GUI).
    #[must_use]
    pub const fn is_legacy(self) -> bool {
        self.major < FIRST_SUPPORTED_MINOR.major
            || (self.major == FIRST_SUPPORTED_MINOR.major
                && self.minor < FIRST_SUPPORTED_MINOR.minor)
    }
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for PhpVersion {
    type Err = CoreError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.is_empty() {
            return Err(err(input, PhpVersionErrorReason::Empty));
        }

        let rest: &str = match input.as_bytes().first() {
            Some(b) if !b.is_ascii() => {
                return Err(err(input, PhpVersionErrorReason::UnsupportedPrefix));
            }
            Some(b) if b.is_ascii_alphabetic() => match input.as_bytes().get(..3) {
                Some(b3) if b3.eq_ignore_ascii_case(b"php") => {
                    let after = input.split_at(3).1;
                    if after
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphabetic)
                    {
                        return Err(err(input, PhpVersionErrorReason::UnsupportedPrefix));
                    }
                    after
                }
                _ => return Err(err(input, PhpVersionErrorReason::UnsupportedPrefix)),
            },
            _ => input,
        };

        let (major_str, minor_str) = rest
            .split_once('.')
            .ok_or_else(|| err(input, PhpVersionErrorReason::MissingMinor))?;

        if major_str.is_empty()
            || minor_str.is_empty()
            || !major_str.bytes().all(|b| b.is_ascii_digit())
            || !minor_str.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(err(input, PhpVersionErrorReason::NonNumeric));
        }

        let major: u16 = major_str
            .parse()
            .map_err(|_| err(input, PhpVersionErrorReason::NonNumeric))?;
        let minor: u16 = minor_str
            .parse()
            .map_err(|_| err(input, PhpVersionErrorReason::NonNumeric))?;

        if !(5..=9).contains(&major) {
            return Err(err(input, PhpVersionErrorReason::MajorOutOfRange));
        }
        if minor > 99 {
            return Err(err(input, PhpVersionErrorReason::MinorOutOfRange));
        }

        Ok(Self::new(major as u8, minor as u8))
    }
}

fn err(input: &str, reason: PhpVersionErrorReason) -> CoreError {
    CoreError::InvalidPhpVersion {
        input: input.to_owned(),
        reason,
    }
}

impl serde::Serialize for PhpVersion {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for PhpVersion {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = PhpVersion;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#"a PHP version string like "8.3" or "php8.3""#)
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PhpVersion, E> {
                v.parse::<PhpVersion>().map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_str(V)
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
    use serde_test::{assert_de_tokens_error, assert_tokens, Token};

    #[test]
    fn parse_accepts_canonical() {
        for (s, m, n) in [
            ("8.3", 8, 3),
            ("7.4", 7, 4),
            ("5.6", 5, 6),
            ("5.0", 5, 0),
            ("9.0", 9, 0),
            ("8.10", 8, 10),
            ("8.99", 8, 99),
        ] {
            let v: PhpVersion = s.parse().unwrap();
            assert_eq!(v, PhpVersion::new(m, n), "input {s}");
        }
    }

    #[test]
    fn parse_accepts_php_prefix_case_insensitive() {
        for s in ["php8.3", "PHP8.3", "Php8.3", "pHp8.3", "PHp8.3"] {
            let v: PhpVersion = s.parse().unwrap();
            assert_eq!(v, PhpVersion::new(8, 3), "input {s}");
        }
    }

    #[test]
    fn parse_classifies_each_reason_pinned() {
        use PhpVersionErrorReason::*;
        let cases: &[(&str, PhpVersionErrorReason)] = &[
            ("", Empty),
            ("8", MissingMinor),
            ("php", MissingMinor),
            ("php8", MissingMinor),
            ("php.", NonNumeric),
            ("8.", NonNumeric),
            (".3", NonNumeric),
            ("8.3 ", NonNumeric),
            ("  8.3", NonNumeric),
            ("+8.3", NonNumeric),
            ("-8.3", NonNumeric),
            ("v8.3", UnsupportedPrefix),
            ("py8.3", UnsupportedPrefix),
            ("phpa8.3", UnsupportedPrefix),
            ("phpython8.3", UnsupportedPrefix),
            ("phpz", UnsupportedPrefix),
            ("83.0", MajorOutOfRange),
            ("999.0", MajorOutOfRange),
            ("65535.0", MajorOutOfRange),
            ("8.100", MinorOutOfRange),
            ("8.300", MinorOutOfRange),
            ("8.65535", MinorOutOfRange),
            ("8.65536", NonNumeric),
            ("8.99999", NonNumeric),
            ("65536.0", NonNumeric),
            ("99999.0", NonNumeric),
        ];
        for (input, expected) in cases {
            let res = input.parse::<PhpVersion>();
            match res {
                Err(CoreError::InvalidPhpVersion { reason, .. }) => {
                    assert_eq!(reason, *expected, "input {input:?}");
                }
                Err(other) => panic!("unexpected error for {input:?}: {other:?}"),
                Ok(v) => panic!("expected error for {input:?}, got {v}"),
            }
        }
    }

    #[test]
    fn parse_rejects_non_ascii_leading_byte() {
        for s in ["é8.3", "中8.3", "🦀8.3", "\u{200B}8.3", "é", "中"] {
            let res = s.parse::<PhpVersion>();
            match res {
                Err(CoreError::InvalidPhpVersion { reason, .. }) => {
                    assert_eq!(
                        reason,
                        PhpVersionErrorReason::UnsupportedPrefix,
                        "input {s:?}"
                    );
                }
                other => panic!("expected UnsupportedPrefix for {s:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_classifies_combining_accent_as_unsupported_prefix() {
        let res = "a\u{0301}8.3".parse::<PhpVersion>();
        match res {
            Err(CoreError::InvalidPhpVersion { reason, .. }) => {
                assert_eq!(reason, PhpVersionErrorReason::UnsupportedPrefix);
            }
            other => panic!("expected UnsupportedPrefix, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_php_with_extra_letters() {
        for s in ["phpa8.3", "phpython8.3", "phpz"] {
            assert!(
                matches!(
                    s.parse::<PhpVersion>(),
                    Err(CoreError::InvalidPhpVersion {
                        reason: PhpVersionErrorReason::UnsupportedPrefix,
                        ..
                    })
                ),
                "input {s}"
            );
        }
    }

    #[test]
    fn parse_classifies_overflow_uniform() {
        use PhpVersionErrorReason::*;
        assert_eq!(
            "8.99".parse::<PhpVersion>().unwrap(),
            PhpVersion::new(8, 99)
        );
        for (s, r) in [
            ("8.100", MinorOutOfRange),
            ("8.300", MinorOutOfRange),
            ("8.65535", MinorOutOfRange),
            ("83.0", MajorOutOfRange),
            ("999.0", MajorOutOfRange),
            ("65535.0", MajorOutOfRange),
        ] {
            assert!(
                matches!(s.parse::<PhpVersion>(), Err(CoreError::InvalidPhpVersion { reason, .. }) if reason == r),
                "input {s} expected {r:?}"
            );
        }
        for s in ["8.65536", "8.99999", "65536.0", "99999.0"] {
            assert!(
                matches!(
                    s.parse::<PhpVersion>(),
                    Err(CoreError::InvalidPhpVersion {
                        reason: PhpVersionErrorReason::NonNumeric,
                        ..
                    })
                ),
                "input {s}"
            );
        }
    }

    #[test]
    fn parse_does_not_panic_on_multibyte() {
        for s in [
            "é8.3",
            "中8.3",
            "🦀8.3",
            "\u{200B}",
            "a\u{0301}8.3",
            "é",
            "中",
        ] {
            let _ = s.parse::<PhpVersion>();
        }
    }

    #[test]
    fn display_emits_dotted() {
        assert_eq!(PhpVersion::new(8, 3).to_string(), "8.3");
        assert_eq!(PhpVersion::new(8, 10).to_string(), "8.10");
        assert_eq!(PhpVersion::new(5, 0).to_string(), "5.0");
    }

    #[test]
    fn ordering_pinned() {
        let php74 = PhpVersion::new(7, 4);
        let php80 = PhpVersion::new(8, 0);
        let php89 = PhpVersion::new(8, 9);
        let php8_dot_10 = PhpVersion::new(8, 10);
        let php899 = PhpVersion::new(8, 99);
        assert!(php74 < php80);
        assert!(php80 < php89);
        assert!(
            php89 < php8_dot_10,
            "(8,9) < (8,10) — guards numeric (not lex) ordering"
        );
        assert!(php8_dot_10 < php899);
    }

    #[test]
    fn is_legacy_splits_at_first_supported_minor() {
        for (m, n) in [(5, 6), (7, 0), (7, 4), (8, 0), (8, 1)] {
            assert!(
                PhpVersion::new(m, n).is_legacy(),
                "{m}.{n} should be legacy"
            );
        }
        for (m, n) in [(8, 2), (8, 3), (8, 4), (8, 5), (9, 0)] {
            assert!(
                !PhpVersion::new(m, n).is_legacy(),
                "{m}.{n} should not be legacy"
            );
        }
        assert!(!FIRST_SUPPORTED_MINOR.is_legacy());
    }

    #[test]
    fn round_trip_canonicalises() {
        let v: PhpVersion = "8.00".parse().unwrap();
        assert_eq!(v, PhpVersion::new(8, 0));
        assert_eq!(
            v.to_string(),
            "8.0",
            "redundant zeros are lost on round-trip"
        );
    }

    #[test]
    fn serde_wire_shape_string() {
        assert_tokens(&PhpVersion::new(8, 3), &[Token::Str("8.3")]);
    }

    #[test]
    fn serde_rejects_json_number() {
        let res: Result<PhpVersion, _> = serde_json::from_str("8.3");
        assert!(res.is_err());
    }

    #[test]
    fn serde_rejects_json_int() {
        let res: Result<PhpVersion, _> = serde_json::from_str("8");
        assert!(res.is_err());
    }

    #[test]
    fn serde_visitor_expecting_hit() {
        assert_de_tokens_error::<PhpVersion>(
            &[Token::Unit],
            "invalid type: unit value, expected a PHP version string like \"8.3\" or \"php8.3\"",
        );
    }
}
