//! Build HTTP → HTTPS redirect URIs.

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

/// Strip the trailing `:port` from `host`, handling IPv6 literals `[...]`.
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6. The closing `]` ends the host portion.
        if let Some(end) = rest.find(']') {
            // Keep the `[...]` bracketed form.
            return host
                .get(..end + 2)
                .unwrap_or(host)
                .trim_end_matches(':')
                .trim_end_matches('[')
                .get(..)
                .map_or(host, |_| {
                    // Simpler: return up to and including `]`.
                    host.get(..end + 2).unwrap_or(host)
                });
        }
        return host;
    }
    // Plain host:port — split at the last `:` only if there's exactly one.
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
            // (host, path_and_query, https_port, expected)
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
