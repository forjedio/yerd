//! Pure helpers for the tunnel origin listener: public `Host` normalization,
//! the HTTP basic-auth share-password check, and keeping a protected share's
//! responses out of shared caches.

use http::header::{HeaderMap, HeaderValue, CACHE_CONTROL};

/// Cache-control headers that CDNs read ahead of `Cache-Control` (RFC 9213 and
/// vendor variants), so they could otherwise re-enable edge caching.
const SHARED_CACHE_OVERRIDES: &[&str] = &[
    "cdn-cache-control",
    "cloudflare-cdn-cache-control",
    "surrogate-control",
];

/// Keep a password-protected share's response out of shared caches such as
/// the tunnel's CDN edge, whose cache key ignores `Authorization`.
///
/// RFC 9111 section 3.5 lets a shared cache store a response to an
/// authenticated request when it carries `public`, `s-maxage` or
/// `must-revalidate`. A bare `private` forbids that whatever else is present,
/// so it is the load-bearing part: it is added unless `no-store` or a bare
/// `private` is already there. `public`, `s-maxage` and a field-scoped
/// `private="..."` are dropped as well, so the header does not contradict
/// itself and a cache that reads only the first `private` cannot miss the bare
/// one. Directives are split on commas outside quoted strings, so a token
/// inside a quoted field list never counts. A value that is not valid UTF-8 or
/// leaves a quoted string open cannot be parsed safely, so the whole header
/// becomes the strictest `private, no-store`. The CDN-specific override
/// headers are removed. Browser caching is otherwise left as the site set it.
pub fn keep_out_of_shared_caches(headers: &mut HeaderMap) {
    for name in SHARED_CACHE_OVERRIDES {
        headers.remove(*name);
    }
    let mut directives: Vec<String> = Vec::new();
    for value in &headers.get_all(CACHE_CONTROL) {
        let Some(split) = value.to_str().ok().and_then(split_directives) else {
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
            return;
        };
        directives.extend(
            split
                .into_iter()
                .filter(|d| !is_shared_cache_grant(d))
                .map(str::to_owned),
        );
    }
    let has_bare = |want: &str| directives.iter().any(|d| d.eq_ignore_ascii_case(want));
    if !has_bare("no-store") && !has_bare("private") {
        directives.push("private".to_owned());
    }
    let value = HeaderValue::from_str(&directives.join(", "))
        .unwrap_or_else(|_| HeaderValue::from_static("private, no-store"));
    headers.insert(CACHE_CONTROL, value);
}

/// Split a `Cache-Control` value into trimmed, non-empty directives at commas
/// that sit outside a quoted string (RFC 9110 `quoted-string`, with `\`
/// escapes). Returns `None` when a quoted string is never closed.
fn split_directives(value: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                out.push(value.get(start..i).unwrap_or_default());
                start = i + 1;
            }
            _ => {}
        }
    }
    if in_quotes {
        return None;
    }
    out.push(value.get(start..).unwrap_or_default());
    Some(
        out.into_iter()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .collect(),
    )
}

fn is_shared_cache_grant(directive: &str) -> bool {
    let (name, scoped) = match directive.split_once('=') {
        Some((name, _)) => (name.trim(), true),
        None => (directive.trim(), false),
    };
    name.eq_ignore_ascii_case("public")
        || name.eq_ignore_ascii_case("s-maxage")
        || (scoped && name.eq_ignore_ascii_case("private"))
}

/// Normalize a `Host` header value into a tunnel route key: strip a `:port`
/// suffix and a trailing dot, and lowercase. Returns `None` for an empty value
/// or one containing whitespace.
#[must_use]
pub fn normalize_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let without_port = match trimmed.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return None,
        None => trimmed,
    };
    let host = without_port.strip_suffix('.').unwrap_or(without_port);
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Extract the password from an `Authorization: Basic <base64(user:pass)>`
/// header. The scheme is matched case-insensitively and the credentials are
/// split at the first `:`. **The username is ignored**: a share password works
/// with any username. Returns `None` for any other scheme, undecodable base64,
/// non-UTF-8 credentials, a missing `:`, or an empty password.
#[must_use]
pub fn basic_auth_password(header: &str) -> Option<String> {
    let (scheme, token) = header.trim().split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = crate::pure::base64::decode(token.trim())?;
    let credentials = String::from_utf8(decoded).ok()?;
    let (_user, password) = credentials.split_once(':')?;
    if password.is_empty() {
        return None;
    }
    Some(password.to_owned())
}

/// Constant-time comparison of a supplied share password against the expected
/// one, so the check does not leak how many leading bytes matched.
#[must_use]
pub fn password_matches(given: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    given.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn keep_out_of_shared_caches_table() {
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
        let cases: &[Case<'_>] = &[
            ("no cache-control becomes private", &[], "private"),
            (
                "public is swapped for private",
                &[("cache-control", "public, max-age=60")],
                "max-age=60, private",
            ),
            (
                "s-maxage is dropped",
                &[("cache-control", "max-age=60, S-MAXAGE=600")],
                "max-age=60, private",
            ),
            (
                "no-store is kept as is",
                &[("cache-control", "no-store")],
                "no-store",
            ),
            (
                "a bare private is kept as is",
                &[("cache-control", "private, max-age=60")],
                "private, max-age=60",
            ),
            (
                "a field-scoped private becomes a bare private",
                &[("cache-control", "private=\"set-cookie\", max-age=60")],
                "max-age=60, private",
            ),
            (
                "a field-scoped private beside a bare one is dropped",
                &[("cache-control", "private=\"set-cookie\", private")],
                "private",
            ),
            (
                "an unclosed quoted string falls back to no-store",
                &[("cache-control", "no-cache=\"a, public, max-age=600")],
                "private, no-store",
            ),
            (
                "a dangling escape inside quotes falls back to no-store",
                &[("cache-control", "max-age=5, no-cache=\"a\\")],
                "private, no-store",
            ),
            (
                "a private inside a quoted list does not count",
                &[(
                    "cache-control",
                    "no-cache=\"a, private, b\", public, max-age=600",
                )],
                "no-cache=\"a, private, b\", max-age=600, private",
            ),
            (
                "an escaped quote stays inside the quoted string",
                &[("cache-control", "no-cache=\"a\\\", no-store\", max-age=5")],
                "no-cache=\"a\\\", no-store\", max-age=5, private",
            ),
            (
                "must-revalidate alone still gains private",
                &[("cache-control", "must-revalidate, max-age=600")],
                "must-revalidate, max-age=600, private",
            ),
            (
                "repeated headers are merged",
                &[("cache-control", "public"), ("cache-control", "max-age=5")],
                "max-age=5, private",
            ),
        ];
        for (label, given, want) in cases {
            let mut headers = HeaderMap::new();
            for (name, value) in *given {
                headers.append(*name, HeaderValue::from_str(value).unwrap());
            }
            keep_out_of_shared_caches(&mut headers);
            let got: Vec<&str> = headers
                .get_all(CACHE_CONTROL)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect();
            assert_eq!(got, vec![*want], "{label}");
        }
    }

    #[test]
    fn keep_out_of_shared_caches_falls_back_to_no_store_for_non_utf8() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CACHE_CONTROL,
            HeaderValue::from_bytes(b"no-store, x=\xff").unwrap(),
        );
        keep_out_of_shared_caches(&mut headers);
        assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "private, no-store");
    }

    #[test]
    fn keep_out_of_shared_caches_removes_cdn_overrides() {
        let mut headers = HeaderMap::new();
        for name in [
            "cdn-cache-control",
            "Cloudflare-CDN-Cache-Control",
            "surrogate-control",
        ] {
            headers.insert(
                http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_static("max-age=600"),
            );
        }
        headers.insert("x-other", HeaderValue::from_static("kept"));
        keep_out_of_shared_caches(&mut headers);
        assert!(!headers.contains_key("cdn-cache-control"));
        assert!(!headers.contains_key("cloudflare-cdn-cache-control"));
        assert!(!headers.contains_key("surrogate-control"));
        assert_eq!(headers.get("x-other").unwrap(), "kept");
    }

    #[test]
    fn normalize_host_table() {
        for (raw, expected) in [
            ("app.example.com", Some("app.example.com")),
            ("A.Example.com.", Some("a.example.com")),
            ("app.example.com:443", Some("app.example.com")),
            ("App.Example.Com.:8080", Some("app.example.com")),
            (" app.example.com ", Some("app.example.com")),
            ("", None),
            ("   ", None),
            (":443", None),
            (".", None),
            ("app example.com", None),
            ("app.example.com:", None),
            ("app.example.com:abc", None),
        ] {
            assert_eq!(normalize_host(raw).as_deref(), expected, "input {raw:?}");
        }
    }

    #[test]
    fn basic_auth_password_table() {
        for (header, expected) in [
            ("Basic dXNlcjpzZWNyZXQ=", Some("secret")),
            ("basic dXNlcjpzZWNyZXQ=", Some("secret")),
            ("BASIC   dXNlcjpzZWNyZXQ=", Some("secret")),
            ("Basic OnNlY3JldA==", Some("secret")),
            ("Basic dTpwOmFzcw==", Some("p:ass")),
            ("basic xyz", None),
            ("Basic dXNlcnNlY3JldA==", None),
            ("Basic dXNlcjo=", None),
            ("Basic not*base64", None),
            ("Bearer dXNlcjpzZWNyZXQ=", None),
            ("Basic", None),
            ("", None),
        ] {
            assert_eq!(
                basic_auth_password(header).as_deref(),
                expected,
                "header {header:?}"
            );
        }
    }

    #[test]
    fn password_matches_table() {
        for (given, expected, ok) in [
            ("secret", "secret", true),
            ("secreT", "secret", false),
            ("secre", "secret", false),
            ("secrets", "secret", false),
            ("", "secret", false),
            ("p\u{e9}ss", "p\u{e9}ss", true),
        ] {
            assert_eq!(
                password_matches(given, expected),
                ok,
                "{given:?} vs {expected:?}"
            );
        }
    }
}
