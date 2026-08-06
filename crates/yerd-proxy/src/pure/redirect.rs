//! Build the `Location` values Yerd's own redirects use: the HTTP → HTTPS
//! upgrade (absolute URI) and the trailing-slash directory redirect
//! (path-relative).

/// Build an HTTPS redirect URI from an inbound HTTP request.
///
/// - Strips any inbound port from `host` (handles `[::1]:80`, `host:80`).
/// - Lowercases the result.
/// - Appends `:https_port` only when it isn't 443.
/// - IPv6 hosts are formatted per RFC 3986 (`[…]:port`).
/// - If `path_and_query` is empty, defaults to `/`.
#[must_use]
pub fn build_redirect_uri(host: &str, path_and_query: &str, https_port: u16) -> String {
    let bare_host = strip_port(host);
    let host_lower = bare_host.to_ascii_lowercase();
    let pq = if path_and_query.is_empty() {
        "/"
    } else {
        path_and_query
    };
    if https_port == 443 {
        format!("https://{host_lower}{pq}")
    } else {
        format!("https://{host_lower}:{https_port}{pq}")
    }
}

/// The trailing-slash `Location` for a directory request that arrived without
/// one: `/sub?x=1` -> `/sub/?x=1`. Path-relative, so it is scheme- and
/// host-agnostic and works behind either listener.
///
/// An empty `path_and_query` degrades to `/`, and a path that already ends in
/// `/` is returned unchanged, so the caller can never build a redirect loop.
#[must_use]
pub fn directory_redirect_location(path_and_query: &str) -> String {
    let (path, query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };
    let mut out = if path.is_empty() {
        String::from("/")
    } else if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    };
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    out
}

/// Strip the trailing `:port` from `host`, handling IPv6 literals `[...]`.
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => host.get(..end + 2).unwrap_or(host),
            None => host,
        };
    }
    let colons = host.bytes().filter(|&b| b == b':').count();
    if colons == 1 {
        host.split(':').next().unwrap_or(host)
    } else {
        host
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

    #[test]
    fn build_table() {
        let cases: &[(&str, &str, u16, &str)] = &[
            ("app.test", "/foo", 443, "https://app.test/foo"),
            ("app.test", "/foo", 8443, "https://app.test:8443/foo"),
            ("app.test:80", "/foo?a=1", 443, "https://app.test/foo?a=1"),
            ("APP.TEST", "/", 443, "https://app.test/"),
            ("app.test", "", 443, "https://app.test/"),
            ("app.test", "", 8443, "https://app.test:8443/"),
            ("[::1]:80", "/x", 443, "https://[::1]/x"),
            ("[::1]", "/x", 8443, "https://[::1]:8443/x"),
            ("[2001:db8::1]:80", "/", 443, "https://[2001:db8::1]/"),
        ];
        for (host, pq, port, want) in cases {
            assert_eq!(
                build_redirect_uri(host, pq, *port),
                *want,
                "case: host={host:?} pq={pq:?} port={port}"
            );
        }
    }

    #[test]
    fn directory_redirect_table() {
        let cases: &[(&str, &str)] = &[
            ("/sub", "/sub/"),
            ("/sub?x=1", "/sub/?x=1"),
            ("/a/b", "/a/b/"),
            ("/sub/", "/sub/"),
            ("/sub/?x=1", "/sub/?x=1"),
            ("/sub?", "/sub/?"),
            ("", "/"),
        ];
        for (pq, want) in cases {
            assert_eq!(directory_redirect_location(pq), *want, "case: pq={pq:?}");
        }
    }

    #[test]
    fn strip_port_ipv6_no_port() {
        assert_eq!(strip_port("[::1]"), "[::1]");
    }

    #[test]
    fn strip_port_ipv6_with_port() {
        assert_eq!(strip_port("[::1]:8443"), "[::1]");
    }

    #[test]
    fn strip_port_plain() {
        assert_eq!(strip_port("app.test:80"), "app.test");
        assert_eq!(strip_port("app.test"), "app.test");
    }
}
