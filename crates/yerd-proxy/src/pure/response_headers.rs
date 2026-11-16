//! Pure application of a site's custom response headers.
//!
//! Semantics are **replace**: each configured header removes every value the
//! response already carried under that name (compared case-insensitively, since
//! `HeaderName` is lowercase-normalised) and inserts the configured one. Only
//! the header map is touched, so a streaming body is never buffered.

use http::header::{HeaderName, HeaderValue};
use http::Response;
use yerd_core::ResponseHeader;

/// Set every header in `configured` on `resp`, replacing same-named headers.
///
/// A header that fails to convert is skipped. That cannot happen for a value
/// built by [`ResponseHeader::new`], whose validation matches what `http`
/// accepts, but it keeps this function total.
pub fn apply<B>(resp: &mut Response<B>, configured: &[ResponseHeader]) {
    if configured.is_empty() {
        return;
    }
    let headers = resp.headers_mut();
    for h in configured {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(h.name().as_bytes()),
            HeaderValue::from_str(h.value()),
        ) else {
            continue;
        };
        headers.insert(name, value);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> ResponseHeader {
        ResponseHeader::new(name, value).unwrap()
    }

    fn response(backend: &[(&str, &str)]) -> Response<()> {
        let mut builder = Response::builder();
        for (name, value) in backend {
            builder = builder.header(*name, *value);
        }
        builder.body(()).unwrap()
    }

    fn values(resp: &Response<()>, name: &str) -> Vec<String> {
        resp.headers()
            .get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn applies_configured_headers_with_replace_semantics() {
        type Case<'a> = (
            &'a str,
            &'a [(&'a str, &'a str)],
            &'a [(&'a str, &'a str)],
            &'a [(&'a str, &'a [&'a str])],
        );
        let cases: &[Case<'_>] = &[
            (
                "no configured headers leaves the map alone",
                &[("X-Backend", "kept")],
                &[],
                &[("x-backend", &["kept"])],
            ),
            (
                "adds a new header",
                &[],
                &[("X-Frame-Options", "DENY")],
                &[("x-frame-options", &["DENY"])],
            ),
            (
                "replaces a single backend value",
                &[("Cache-Control", "public")],
                &[("Cache-Control", "no-store")],
                &[("cache-control", &["no-store"])],
            ),
            (
                "replaces repeated backend values with one",
                &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Access-Control-Allow-Origin", "https://x.test"),
                ],
                &[("Access-Control-Allow-Origin", "https://app.test")],
                &[("access-control-allow-origin", &["https://app.test"])],
            ),
            (
                "matches backend names ignoring case",
                &[("access-control-allow-origin", "*")],
                &[("Access-Control-Allow-Origin", "https://app.test")],
                &[("access-control-allow-origin", &["https://app.test"])],
            ),
            (
                "applies several headers and keeps unrelated ones",
                &[("X-Backend", "kept"), ("X-Frame-Options", "SAMEORIGIN")],
                &[("X-Frame-Options", "DENY"), ("X-Custom", "1")],
                &[
                    ("x-backend", &["kept"]),
                    ("x-frame-options", &["DENY"]),
                    ("x-custom", &["1"]),
                ],
            ),
        ];
        for (label, backend, configured, want) in cases {
            let mut resp = response(backend);
            let configured: Vec<ResponseHeader> =
                configured.iter().map(|(n, v)| header(n, v)).collect();
            apply(&mut resp, &configured);
            let total: usize = want.iter().map(|(_, vs)| vs.len()).sum();
            assert_eq!(resp.headers().len(), total, "{label}");
            for (name, expected) in *want {
                assert_eq!(values(&resp, name), *expected, "{label}: {name}");
            }
        }
    }
}
