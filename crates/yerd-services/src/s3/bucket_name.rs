//! S3 general-purpose bucket naming rules, checked before a create is sent.
//!
//! Only bucket creation validates names; drop and stats send the name URI
//! encoded and let the gateway answer `NoSuchBucket`.

use thiserror::Error;

const MIN_LEN: usize = 3;
const MAX_LEN: usize = 63;
const RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];
const RESERVED_SUFFIXES: [&str; 4] = ["-s3alias", "--ol-s3", ".mrap", "--x-s3"];

/// Why a proposed bucket name was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BucketNameError {
    /// Shorter than 3 or longer than 63 characters.
    #[error("bucket names must be 3 to 63 characters long (got {0})")]
    Length(usize),
    /// A character outside lowercase letters, digits, `.` and `-`.
    #[error(
        "bucket names may only contain lowercase letters, digits, hyphens and dots (found {0:?})"
    )]
    InvalidCharacter(char),
    /// The first or last character is not a letter or digit.
    #[error("bucket names must start and end with a lowercase letter or digit")]
    BadEdge,
    /// Two dots in a row.
    #[error("bucket names must not contain two dots in a row")]
    ConsecutiveDots,
    /// Formatted like an IPv4 address.
    #[error("bucket names must not be formatted as an IP address")]
    IpAddress,
    /// Starts with a prefix S3 reserves.
    #[error("bucket names must not start with {0:?}")]
    ReservedPrefix(&'static str),
    /// Ends with a suffix S3 reserves.
    #[error("bucket names must not end with {0:?}")]
    ReservedSuffix(&'static str),
}

/// Check `name` against the S3 general-purpose bucket naming rules.
///
/// # Errors
/// The first [`BucketNameError`] rule the name breaks.
pub fn validate_bucket_name(name: &str) -> Result<(), BucketNameError> {
    let len = name.chars().count();
    if !(MIN_LEN..=MAX_LEN).contains(&len) {
        return Err(BucketNameError::Length(len));
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '-'))
    {
        return Err(BucketNameError::InvalidCharacter(c));
    }
    let edge_ok = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if !edge_ok(name.chars().next()) || !edge_ok(name.chars().last()) {
        return Err(BucketNameError::BadEdge);
    }
    if name.contains("..") {
        return Err(BucketNameError::ConsecutiveDots);
    }
    if looks_like_ipv4(name) {
        return Err(BucketNameError::IpAddress);
    }
    if let Some(prefix) = RESERVED_PREFIXES.iter().find(|p| name.starts_with(*p)) {
        return Err(BucketNameError::ReservedPrefix(prefix));
    }
    if let Some(suffix) = RESERVED_SUFFIXES.iter().find(|s| name.ends_with(*s)) {
        return Err(BucketNameError::ReservedSuffix(suffix));
    }
    Ok(())
}

fn looks_like_ipv4(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        let cases = [
            "abc".to_owned(),
            "a.b-c".to_owned(),
            "my-bucket.v2".to_owned(),
            "user-uploads".to_owned(),
            "health".to_owned(),
            "1.2.3".to_owned(),
            "a".repeat(63),
        ];
        for name in &cases {
            assert_eq!(validate_bucket_name(name), Ok(()), "name {name:?}");
        }
    }

    #[test]
    fn rejects_each_rule() {
        let long = "a".repeat(64);
        let cases: [(&str, BucketNameError); 18] = [
            ("", BucketNameError::Length(0)),
            ("ab", BucketNameError::Length(2)),
            (&long, BucketNameError::Length(64)),
            ("Assets", BucketNameError::InvalidCharacter('A')),
            ("my_bucket", BucketNameError::InvalidCharacter('_')),
            ("a b", BucketNameError::InvalidCharacter(' ')),
            ("a/b", BucketNameError::InvalidCharacter('/')),
            ("bücket", BucketNameError::InvalidCharacter('ü')),
            ("-abc", BucketNameError::BadEdge),
            ("abc-", BucketNameError::BadEdge),
            (".abc", BucketNameError::BadEdge),
            ("abc.", BucketNameError::BadEdge),
            ("a..b", BucketNameError::ConsecutiveDots),
            ("192.168.5.4", BucketNameError::IpAddress),
            ("xn--abc", BucketNameError::ReservedPrefix("xn--")),
            ("sthree-abc", BucketNameError::ReservedPrefix("sthree-")),
            ("abc-s3alias", BucketNameError::ReservedSuffix("-s3alias")),
            ("abc--ol-s3", BucketNameError::ReservedSuffix("--ol-s3")),
        ];
        for (name, want) in cases {
            assert_eq!(validate_bucket_name(name), Err(want), "name {name:?}");
        }
    }

    #[test]
    fn rejects_remaining_reserved_affixes() {
        let cases = [
            (
                "amzn-s3-demo-x",
                BucketNameError::ReservedPrefix("amzn-s3-demo-"),
            ),
            ("abc.mrap", BucketNameError::ReservedSuffix(".mrap")),
            ("abc--x-s3", BucketNameError::ReservedSuffix("--x-s3")),
        ];
        for (name, want) in cases {
            assert_eq!(validate_bucket_name(name), Err(want), "name {name:?}");
        }
    }

    #[test]
    fn messages_are_user_facing() {
        assert_eq!(
            BucketNameError::InvalidCharacter('_').to_string(),
            "bucket names may only contain lowercase letters, digits, hyphens and dots (found '_')"
        );
        assert_eq!(
            BucketNameError::Length(2).to_string(),
            "bucket names must be 3 to 63 characters long (got 2)"
        );
    }
}
