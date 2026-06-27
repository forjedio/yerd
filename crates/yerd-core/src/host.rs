//! Host normalisation for the resolver.
//!
//! This is a private, `pub(crate)` module. Only [`SiteRouter::resolve`]
//! consumes it.
//!
//! [`SiteRouter::resolve`]: crate::SiteRouter::resolve

use std::borrow::Cow;

/// The normalised result of inspecting a raw `Host:` header value.
pub(crate) enum HostKind<'a> {
    /// A lowercase ASCII hostname suitable for TLD matching, with no port and
    /// no trailing dot.
    Hostname(Cow<'a, str>),
    /// The input is not routable (IPv6 literal, non-ASCII, malformed port, etc.).
    Unroutable,
}

/// Normalises a raw host string into a [`HostKind`].
pub(crate) fn normalise(raw: &str) -> HostKind<'_> {
    if raw.is_empty() {
        return HostKind::Unroutable;
    }

    if raw.starts_with('[') {
        return HostKind::Unroutable;
    }

    if raw.bytes().any(|b| !b.is_ascii()) {
        return HostKind::Unroutable;
    }

    let host: &str = match raw.rsplit_once(':') {
        None => raw,
        Some((head, tail)) if tail.is_empty() || tail.bytes().all(|b| b.is_ascii_digit()) => head,
        Some(_) => return HostKind::Unroutable,
    };

    if host.is_empty() {
        return HostKind::Unroutable;
    }

    let host: &str = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return HostKind::Unroutable;
    }

    if host.starts_with('.') {
        return HostKind::Unroutable;
    }

    if host.bytes().any(|b| b.is_ascii_uppercase()) {
        HostKind::Hostname(Cow::Owned(host.to_ascii_lowercase()))
    } else {
        HostKind::Hostname(Cow::Borrowed(host))
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

    fn host_of(raw: &str) -> Cow<'_, str> {
        match normalise(raw) {
            HostKind::Hostname(c) => c,
            HostKind::Unroutable => panic!("expected Hostname for {raw:?}, got Unroutable"),
        }
    }

    fn assert_unroutable(raw: &str) {
        assert!(
            matches!(normalise(raw), HostKind::Unroutable),
            "expected Unroutable for {raw:?}"
        );
    }

    #[test]
    fn normalise_lowercases_ascii_only() {
        assert_eq!(host_of("FOO.TEST"), "foo.test");
        assert_eq!(host_of("Foo.Test"), "foo.test");
    }

    #[test]
    fn normalise_rejects_non_ascii() {
        assert_unroutable("föö.test");
        assert_unroutable("中.test");
    }

    #[test]
    fn normalise_rejects_ipv6_bracketed() {
        for input in ["[::1]", "[::1]:8080", "[fe80::1]"] {
            assert_unroutable(input);
        }
    }

    #[test]
    fn normalise_port_strip_digits_only() {
        assert_eq!(host_of("foo.test:8443"), "foo.test");
        assert_eq!(host_of("foo.test:0"), "foo.test");
    }

    #[test]
    fn normalise_port_strip_rejects_junk() {
        assert_unroutable("foo.test:abc");
        assert_unroutable("foo.test:-80");
    }

    #[test]
    fn normalise_tolerates_trailing_colon() {
        assert_eq!(host_of("foo.test:"), "foo.test");
    }

    #[test]
    fn normalise_rejects_only_port() {
        assert_unroutable(":8080");
    }

    #[test]
    fn normalise_strips_one_trailing_dot() {
        assert_eq!(host_of("foo.test."), "foo.test");
        assert_eq!(host_of("foo.test.:80"), "foo.test");
    }

    #[test]
    fn normalise_rejects_leading_dot() {
        assert_unroutable(".foo.test");
    }

    #[test]
    fn normalise_borrows_when_already_normal() {
        match normalise("foo.test") {
            HostKind::Hostname(Cow::Borrowed(s)) => assert_eq!(s, "foo.test"),
            HostKind::Hostname(Cow::Owned(s)) => panic!("expected Borrowed, got Owned({s:?})"),
            HostKind::Unroutable => panic!("expected Hostname, got Unroutable"),
        }
    }

    #[test]
    fn normalise_rejects_empty() {
        assert_unroutable("");
    }
}
