//! Compose and parse macOS `/etc/resolver/<tld>` files.
//!
//! The file format is documented by `resolver(5)`. Yerd uses the two
//! directives it actually relies on:
//!
//! ```text
//! nameserver <ip>
//! port <number>
//! ```
//!
//! Parsing is tolerant: comments, blank lines, additional directives, and
//! arbitrary whitespace between tokens are all ignored. Structural compare
//! against a freshly-composed file is the basis of macOS `is_installed`.

use std::net::SocketAddr;

/// The two fields Yerd writes into `/etc/resolver/<tld>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverFile {
    /// `nameserver` directive — the IP address only.
    pub nameserver: std::net::IpAddr,
    /// `port` directive (defaults to 53 in `resolver(5)` but Yerd always
    /// writes it explicitly).
    pub port: u16,
}

impl ResolverFile {
    /// Convenience constructor from a `SocketAddr`.
    #[must_use]
    pub fn from_addr(addr: SocketAddr) -> Self {
        Self {
            nameserver: addr.ip(),
            port: addr.port(),
        }
    }
}

/// Compose the file content Yerd writes for `/etc/resolver/<tld>`.
///
/// The result ends in a trailing newline.
#[must_use]
pub fn compose(addr: SocketAddr) -> String {
    format!("nameserver {}\nport {}\n", addr.ip(), addr.port())
}

/// Parse a `/etc/resolver/<tld>` file, returning the structural fields.
///
/// Returns `None` if the file lacks a `nameserver` line. A missing or
/// invalid `port` line resolves to `53` per `resolver(5)`.
#[must_use]
pub fn parse(text: &str) -> Option<ResolverFile> {
    let mut nameserver: Option<std::net::IpAddr> = None;
    let mut port: Option<u16> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let directive = tokens.next().unwrap_or("");
        let value = tokens.next().unwrap_or("");
        match directive {
            "nameserver" => {
                if let Ok(ip) = value.parse::<std::net::IpAddr>() {
                    nameserver = Some(ip);
                }
            }
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    port = Some(p);
                }
            }
            _ => {}
        }
    }
    Some(ResolverFile {
        nameserver: nameserver?,
        port: port.unwrap_or(53),
    })
}

/// True iff `text` parses to a `ResolverFile` equal to `expected`. Used by
/// macOS `is_installed` so the probe ignores comments and ordering.
#[must_use]
pub fn matches(text: &str, expected: SocketAddr) -> bool {
    parse(text) == Some(ResolverFile::from_addr(expected))
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn compose_emits_two_lines_with_trailing_newline() {
        let s = compose(loopback(53));
        assert_eq!(s, "nameserver 127.0.0.1\nport 53\n");
    }

    #[test]
    fn parse_roundtrips_compose() {
        let addr = loopback(5353);
        let s = compose(addr);
        assert_eq!(parse(&s), Some(ResolverFile::from_addr(addr)));
    }

    #[test]
    fn parse_tolerates_extra_whitespace_and_comments() {
        let text = "# yerd-managed\n\n   nameserver    127.0.0.1   \nport\t53\n";
        let r = parse(text).unwrap();
        assert_eq!(r.nameserver, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(r.port, 53);
    }

    #[test]
    fn parse_missing_port_defaults_to_53() {
        let text = "nameserver 127.0.0.1\n";
        let r = parse(text).unwrap();
        assert_eq!(r.port, 53);
    }

    #[test]
    fn parse_missing_nameserver_returns_none() {
        let text = "port 53\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn parse_invalid_nameserver_returns_none() {
        let text = "nameserver bogus\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn matches_ignores_comments() {
        let composed = compose(loopback(53));
        let with_comment = format!("# yerd\n{composed}# trailing\n");
        assert!(matches(&with_comment, loopback(53)));
    }

    #[test]
    fn matches_false_on_port_mismatch() {
        let composed = compose(loopback(53));
        assert!(!matches(&composed, loopback(5353)));
    }

    #[test]
    fn matches_false_on_ip_mismatch() {
        let composed = compose(loopback(53));
        let other: SocketAddr = "192.168.1.1:53".parse().unwrap();
        assert!(!matches(&composed, other));
    }
}
