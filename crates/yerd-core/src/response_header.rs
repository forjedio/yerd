//! Pure per-site custom response headers.
//!
//! A [`ResponseHeader`] is a name and value the proxy sets on every response it
//! produces for a PHP site, replacing any same-named header the response already
//! carried. Like [`RouteRule`](crate::RouteRule) it is pure data with no serde
//! impls: `yerd-config` and `yerd-ipc` own their own wire structs.
//!
//! Validation is hand-rolled over bytes so this crate needs no `http`
//! dependency. The name must be an RFC 9110 token, and the value may hold no
//! control character other than HTAB, which is exactly what the proxy's
//! `http::HeaderValue` accepts, so a validated header always converts there.

use crate::error::{CoreError, ResponseHeaderErrorReason};

/// Header names that cannot be configured: hop-by-hop headers, message
/// framing and `Host`, which Yerd owns, plus headers whose replacement would
/// break the response. Replacing `Set-Cookie` drops every cookie the app sets,
/// `Content-Encoding` and `Content-Range` describe the body bytes, and
/// `Location` would hijack Yerd's own redirects. Any name starting with
/// `proxy-` is reserved as well.
const RESERVED_NAMES: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "content-length",
    "host",
    "set-cookie",
    "content-encoding",
    "content-range",
    "location",
];

/// A custom response header attached to a PHP site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHeader {
    name: String,
    value: String,
}

impl ResponseHeader {
    /// Validates `name` (non-empty RFC 9110 token, not a header Yerd manages)
    /// and `value` (trimmed of surrounding spaces and tabs, non-empty, no
    /// control character other than HTAB). The name keeps the given casing.
    pub fn new(name: &str, value: &str) -> Result<Self, CoreError> {
        let name_err = |r: ResponseHeaderErrorReason| CoreError::InvalidResponseHeader {
            input: name.to_owned(),
            reason: r,
        };
        if name.is_empty() {
            return Err(name_err(ResponseHeaderErrorReason::EmptyName));
        }
        if !name.bytes().all(is_tchar) {
            return Err(name_err(ResponseHeaderErrorReason::InvalidName));
        }
        if is_reserved(name) {
            return Err(name_err(ResponseHeaderErrorReason::ReservedName));
        }

        let value_err = |r: ResponseHeaderErrorReason| CoreError::InvalidResponseHeader {
            input: value.to_owned(),
            reason: r,
        };
        let trimmed = value.trim_matches(|c| c == ' ' || c == '\t');
        if trimmed.is_empty() {
            return Err(value_err(ResponseHeaderErrorReason::EmptyValue));
        }
        if trimmed
            .bytes()
            .any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
        {
            return Err(value_err(ResponseHeaderErrorReason::InvalidValue));
        }

        Ok(Self {
            name: name.to_owned(),
            value: trimmed.to_owned(),
        })
    }

    /// The header name, in the casing the user gave.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The trimmed header value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether this header's name equals `other`, ignoring ASCII case.
    #[must_use]
    pub fn has_name(&self, other: &str) -> bool {
        self.name.eq_ignore_ascii_case(other)
    }
}

/// RFC 9110 `tchar`.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn is_reserved(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RESERVED_NAMES.contains(&lower.as_str()) || lower.starts_with("proxy-")
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

    fn reason_of(name: &str, value: &str) -> ResponseHeaderErrorReason {
        match ResponseHeader::new(name, value).unwrap_err() {
            CoreError::InvalidResponseHeader { reason, .. } => reason,
            other => panic!("expected InvalidResponseHeader for {name:?}, got {other:?}"),
        }
    }

    #[test]
    fn accepts_common_names() {
        for name in [
            "Access-Control-Allow-Origin",
            "X-Frame-Options",
            "Cache-Control",
            "x_custom",
            "Content-Type",
        ] {
            let h = ResponseHeader::new(name, "v").unwrap();
            assert_eq!(h.name(), name);
            assert_eq!(h.value(), "v");
        }
    }

    #[test]
    fn rejects_bad_names_with_reasons() {
        let cases: &[(&str, ResponseHeaderErrorReason)] = &[
            ("", ResponseHeaderErrorReason::EmptyName),
            ("X Foo", ResponseHeaderErrorReason::InvalidName),
            ("X:Foo", ResponseHeaderErrorReason::InvalidName),
            ("X(Foo)", ResponseHeaderErrorReason::InvalidName),
            ("X\u{e9}", ResponseHeaderErrorReason::InvalidName),
            ("X\nFoo", ResponseHeaderErrorReason::InvalidName),
        ];
        for (name, want) in cases {
            assert_eq!(reason_of(name, "v"), *want, "name {name:?}");
        }
    }

    #[test]
    fn rejects_reserved_names_in_any_case() {
        for name in [
            "Connection",
            "KEEP-ALIVE",
            "Transfer-Encoding",
            "upgrade",
            "Te",
            "TRAILER",
            "Content-Length",
            "HOST",
            "Set-Cookie",
            "content-encoding",
            "Content-Range",
            "LOCATION",
            "Proxy-Authenticate",
            "proxy-connection",
        ] {
            assert_eq!(
                reason_of(name, "v"),
                ResponseHeaderErrorReason::ReservedName,
                "name {name:?}"
            );
        }
    }

    #[test]
    fn trims_optional_whitespace_from_values() {
        let h = ResponseHeader::new("X-A", "  *\t").unwrap();
        assert_eq!(h.value(), "*");
    }

    #[test]
    fn rejects_bad_values_with_reasons() {
        let cases: &[(&str, ResponseHeaderErrorReason)] = &[
            ("", ResponseHeaderErrorReason::EmptyValue),
            ("   ", ResponseHeaderErrorReason::EmptyValue),
            ("a\rb", ResponseHeaderErrorReason::InvalidValue),
            ("a\nb", ResponseHeaderErrorReason::InvalidValue),
            ("a\0b", ResponseHeaderErrorReason::InvalidValue),
            ("a\x01b", ResponseHeaderErrorReason::InvalidValue),
            ("a\x7fb", ResponseHeaderErrorReason::InvalidValue),
        ];
        for (value, want) in cases {
            assert_eq!(reason_of("X-A", value), *want, "value {value:?}");
        }
    }

    #[test]
    fn accepts_inner_tab_and_utf8_values() {
        for value in ["a\tb", "caf\u{e9}"] {
            let h = ResponseHeader::new("X-A", value).unwrap();
            assert_eq!(h.value(), value);
        }
    }

    #[test]
    fn has_name_ignores_ascii_case() {
        let h = ResponseHeader::new("Access-Control-Allow-Origin", "*").unwrap();
        assert!(h.has_name("access-control-allow-origin"));
        assert!(h.has_name("ACCESS-CONTROL-ALLOW-ORIGIN"));
        assert!(!h.has_name("Access-Control-Allow-Methods"));
    }
}
