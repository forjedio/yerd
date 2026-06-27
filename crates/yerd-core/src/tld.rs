//! Top-level domain newtype.
//!
//! `Tld` is a validated, lowercased, ASCII-only DNS suffix used by the
//! [`SiteRouter`](crate::SiteRouter) for TLD enforcement.

use std::fmt;
use std::str::FromStr;

use crate::error::{CoreError, TldErrorReason};

/// A validated DNS suffix (e.g. `"test"`, `"dev.local"`).
///
/// Always stored lowercased, ASCII-only, with no leading or trailing dot.
/// Construct via [`Tld::new`] or [`Tld::default`] (which yields the canonical
/// `.test` TLD).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tld(String);

impl Tld {
    /// Validates and constructs from a `&str`.
    pub fn new(s: &str) -> Result<Self, CoreError> {
        validate(s).map(Self)
    }

    /// Returns the validated TLD as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Tld {
    /// The `.test` TLD (Yerd's default).
    fn default() -> Self {
        Self(String::from("test"))
    }
}

impl fmt::Display for Tld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Tld {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl serde::Serialize for Tld {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Tld {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Tld;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a DNS TLD string like \"test\" or \"dev.local\"")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Tld, E> {
                Tld::new(v).map_err(serde::de::Error::custom)
            }
        }
        de.deserialize_str(V)
    }
}

fn err(input: &str, reason: TldErrorReason) -> CoreError {
    CoreError::InvalidTld {
        input: input.to_owned(),
        reason,
    }
}

/// Pinned, ordered validation algorithm. The per-step logic lives in
/// [`validate_steps`]; this wrapper attaches `raw` to any failure reason.
fn validate(raw: &str) -> Result<String, CoreError> {
    validate_steps(raw).map_err(|reason| err(raw, reason))
}

fn validate_steps(raw: &str) -> Result<String, TldErrorReason> {
    if raw.is_empty() {
        return Err(TldErrorReason::Empty);
    }

    let stripped: &str = raw.strip_suffix('.').unwrap_or(raw);
    if stripped.is_empty() || stripped.ends_with('.') || stripped.starts_with('.') {
        return Err(TldErrorReason::LeadingOrTrailingDot);
    }

    if stripped.len() > 253 {
        return Err(TldErrorReason::TooLong);
    }

    validate_charset(stripped)?;

    let lowered = stripped.to_ascii_lowercase();

    for label in lowered.split('.') {
        validate_label(label)?;
    }

    Ok(lowered)
}

/// Every byte must be ASCII and non-whitespace.
fn validate_charset(stripped: &str) -> Result<(), TldErrorReason> {
    for &b in stripped.as_bytes() {
        if !b.is_ascii() {
            return Err(TldErrorReason::NonAscii);
        }
        if b.is_ascii_whitespace() {
            return Err(TldErrorReason::ContainsWhitespace);
        }
    }
    Ok(())
}

/// One DNS label - non-empty, ≤ 63 octets, `[a-z0-9-]` only (input is
/// already lowercased), and no leading/trailing hyphen.
fn validate_label(label: &str) -> Result<(), TldErrorReason> {
    if label.is_empty() {
        return Err(TldErrorReason::ConsecutiveDots);
    }
    if label.len() > 63 {
        return Err(TldErrorReason::LabelTooLong);
    }
    for &b in label.as_bytes() {
        let ok = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
        if !ok {
            return Err(TldErrorReason::InvalidCharacter);
        }
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(TldErrorReason::LeadingOrTrailingHyphen);
    }
    Ok(())
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
    use serde_test::{assert_tokens, Token};

    #[test]
    fn new_accepts_valid() {
        for (input, expected) in [
            ("test", "test"),
            ("localhost", "localhost"),
            ("dev.local", "dev.local"),
            ("TEST", "test"),
        ] {
            let t = Tld::new(input).unwrap();
            assert_eq!(t.as_str(), expected, "input {input:?}");
        }
    }

    #[test]
    fn new_strips_one_trailing_dot() {
        assert_eq!(Tld::new("test.").unwrap().as_str(), "test");
    }

    #[test]
    fn new_rejects_each_reason() {
        use TldErrorReason::*;
        let cases: &[(&str, TldErrorReason)] = &[
            ("", Empty),
            (".", LeadingOrTrailingDot),
            (".test", LeadingOrTrailingDot),
            ("test..", LeadingOrTrailingDot),
            ("a..b", ConsecutiveDots),
            ("foo...bar", ConsecutiveDots),
            ("te st", ContainsWhitespace),
            ("te\tst", ContainsWhitespace),
            ("tëst", NonAscii),
            ("测试", NonAscii),
            ("te_st", InvalidCharacter),
            ("te$st", InvalidCharacter),
            ("-foo", LeadingOrTrailingHyphen),
            ("foo-", LeadingOrTrailingHyphen),
            ("foo.bar-", LeadingOrTrailingHyphen),
        ];
        for (input, expected) in cases {
            match Tld::new(input) {
                Err(CoreError::InvalidTld { reason, .. }) => {
                    assert_eq!(reason, *expected, "input {input:?}");
                }
                other => panic!("input {input:?}: expected {expected:?}, got {other:?}"),
            }
        }

        let long_label = "a".repeat(64);
        match Tld::new(&long_label) {
            Err(CoreError::InvalidTld {
                reason: LabelTooLong,
                ..
            }) => {}
            other => panic!("LabelTooLong expected, got {other:?}"),
        }
        let too_long = "a".repeat(254);
        match Tld::new(&too_long) {
            Err(CoreError::InvalidTld {
                reason: TooLong, ..
            }) => {}
            other => panic!("TooLong expected, got {other:?}"),
        }
    }

    #[test]
    fn serde_wire_shape_string() {
        let t = Tld::new("test").unwrap();
        assert_tokens(&t, &[Token::Str("test")]);
    }

    #[test]
    fn serde_rejects_invalid_on_deserialize() {
        let res: Result<Tld, _> = serde_json::from_str("\"\"");
        assert!(res.is_err());
    }

    #[test]
    fn default_is_test_and_matches_new() {
        let d = Tld::default();
        let n = Tld::new("test").unwrap();
        assert_eq!(d.as_str(), "test");
        assert_eq!(d, n);
        assert_eq!(validate("test").unwrap(), "test");
    }

    #[test]
    fn display_round_trip() {
        let t = Tld::new("dev.local").unwrap();
        assert_eq!(t.to_string(), "dev.local");
        assert_eq!(Tld::new(&t.to_string()).unwrap(), t);
    }
}
