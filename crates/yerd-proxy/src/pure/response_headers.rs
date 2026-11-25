//! Pure application of a site's or whole-host proxy's custom response headers.
//!
//! Semantics are **replace**: each configured header removes every value the
//! response already carried under that name (compared case-insensitively, since
//! `HeaderName` is lowercase-normalised) and inserts the configured one. Only
//! the header map is touched, so a streaming body is never buffered.

use http::header::{HeaderName, HeaderValue};
use http::Response;
use yerd_core::ResponseHeader;

const CORS_PREFIX: &str = "access-control-";

/// Set every header in `configured` on `resp`, replacing same-named headers.
///
/// A header that fails to convert is skipped. That cannot happen for a value
/// built by [`ResponseHeader::new`], whose validation matches what `http`
/// accepts, but it keeps this function total.
pub fn apply<'a, B>(
    resp: &mut Response<B>,
    configured: impl IntoIterator<Item = &'a ResponseHeader>,
) {
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

/// Set only the CORS (`Access-Control-*`) headers in `configured` on `resp`,
/// replacing same-named headers.
///
/// Used for the share-password challenge: CORS headers are meant to be public,
/// cannot affect caching or the challenge itself, and let a cross-origin
/// caller read the `401` instead of failing CORS. Every other configured
/// header stays behind the password.
pub fn apply_cors<B>(resp: &mut Response<B>, configured: &[ResponseHeader]) {
    apply(resp, configured.iter().filter(|h| is_cors_header(h.name())));
}

fn is_cors_header(name: &str) -> bool {
    name.get(..CORS_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(CORS_PREFIX))
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
    fn apply_cors_sets_only_access_control_headers() {
        type Case<'a> = (
            &'a str,
            &'a [(&'a str, &'a str)],
            &'a [&'a str],
            &'a [&'a str],
        );
        let cases: &[Case<'_>] = &[
            (
                "only CORS headers pass",
                &[
                    ("Access-Control-Allow-Origin", "https://app.test"),
                    ("access-control-allow-credentials", "true"),
                    ("X-Debug-Env", "staging"),
                    ("Cache-Control", "public, max-age=60"),
                    ("CDN-Cache-Control", "max-age=600"),
                    ("WWW-Authenticate", "Bearer"),
                ],
                &[
                    "access-control-allow-origin",
                    "access-control-allow-credentials",
                ],
                &[
                    "x-debug-env",
                    "cache-control",
                    "cdn-cache-control",
                    "www-authenticate",
                ],
            ),
            (
                "a near-miss prefix is not CORS",
                &[
                    ("Access-Control", "x"),
                    ("X-Access-Control-Allow-Origin", "*"),
                ],
                &[],
                &["access-control", "x-access-control-allow-origin"],
            ),
            ("nothing configured", &[], &[], &[]),
        ];
        for (label, configured, present, absent) in cases {
            let mut resp = response(&[]);
            let configured: Vec<ResponseHeader> =
                configured.iter().map(|(n, v)| header(n, v)).collect();
            apply_cors(&mut resp, &configured);
            assert_eq!(resp.headers().len(), present.len(), "{label}");
            for name in *present {
                assert!(resp.headers().contains_key(*name), "{label}: {name}");
            }
            for name in *absent {
                assert!(!resp.headers().contains_key(*name), "{label}: {name}");
            }
        }
    }

    #[test]
    fn apply_cors_replaces_a_same_named_header() {
        let mut resp = response(&[("Access-Control-Allow-Origin", "*")]);
        apply_cors(
            &mut resp,
            &[header("Access-Control-Allow-Origin", "https://app.test")],
        );
        assert_eq!(
            values(&resp, "access-control-allow-origin"),
            vec!["https://app.test"]
        );
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
