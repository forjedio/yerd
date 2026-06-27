//! Compose and match systemd-resolved drop-in files
//! (`/etc/systemd/resolved.conf.d/yerd-<tld>.conf`).
//!
//! The file format is documented by `resolved.conf(5)`:
//!
//! ```text
//! [Resolve]
//! DNS=127.0.0.1:5353
//! Domains=~test
//! ```
//!
//! Yerd's compose function emits exactly that shape. The parser is
//! tolerant of comments, blank lines, and extra unrelated keys so the
//! Linux `is_installed` probe is robust against operator additions.

use std::net::SocketAddr;

/// The two fields Yerd writes into the drop-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropIn {
    /// `DNS=` value as a `SocketAddr`.
    pub dns: SocketAddr,
    /// `Domains=` value with the routing `~` prefix stripped.
    pub domain: String,
}

/// Compose a systemd-resolved drop-in body for `tld` and `addr`.
#[must_use]
pub fn compose(tld: &str, addr: SocketAddr) -> String {
    format!("[Resolve]\nDNS={addr}\nDomains=~{tld}\n")
}

/// Parse a drop-in body.
///
/// Returns `None` if either `DNS=` or `Domains=` is missing or malformed.
#[must_use]
pub fn parse(text: &str) -> Option<DropIn> {
    let mut dns: Option<SocketAddr> = None;
    let mut domain: Option<String> = None;
    let mut in_resolve_section = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_resolve_section = section == "Resolve";
            continue;
        }
        if !in_resolve_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "DNS" => {
                if let Ok(addr) = value.parse::<SocketAddr>() {
                    dns = Some(addr);
                }
            }
            "Domains" => {
                let stripped = value.strip_prefix('~').unwrap_or(value);
                domain = Some(stripped.to_owned());
            }
            _ => {}
        }
    }
    Some(DropIn {
        dns: dns?,
        domain: domain?,
    })
}

/// True iff `text` parses to a `DropIn` equal to `(tld, addr)`.
#[must_use]
pub fn matches(text: &str, tld: &str, addr: SocketAddr) -> bool {
    parse(text)
        == Some(DropIn {
            dns: addr,
            domain: tld.to_owned(),
        })
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

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[test]
    fn compose_shape_matches_expected_lines() {
        let s = compose("test", addr(5353));
        assert_eq!(s, "[Resolve]\nDNS=127.0.0.1:5353\nDomains=~test\n");
    }

    #[test]
    fn parse_roundtrips_compose() {
        let s = compose("test", addr(53));
        let parsed = parse(&s).unwrap();
        assert_eq!(parsed.dns, addr(53));
        assert_eq!(parsed.domain, "test");
    }

    #[test]
    fn parse_tolerates_comments_and_extra_keys() {
        let text = "\
# operator note
[Resolve]
DNS=127.0.0.1:5353
Domains=~test
DNSSEC=allow-downgrade
LLMNR=no
";
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.dns, addr(5353));
        assert_eq!(parsed.domain, "test");
    }

    #[test]
    fn parse_ignores_keys_outside_resolve_section() {
        let text = "\
[Other]
DNS=8.8.8.8:53
Domains=~example

[Resolve]
DNS=127.0.0.1:53
Domains=~test
";
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.dns, addr(53));
        assert_eq!(parsed.domain, "test");
    }

    #[test]
    fn parse_missing_dns_returns_none() {
        let text = "[Resolve]\nDomains=~test\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn parse_missing_domains_returns_none() {
        let text = "[Resolve]\nDNS=127.0.0.1:53\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn matches_ignores_comments() {
        let s = format!("# managed by yerd\n{}", compose("test", addr(53)));
        assert!(matches(&s, "test", addr(53)));
    }

    #[test]
    fn matches_false_on_tld_mismatch() {
        let s = compose("test", addr(53));
        assert!(!matches(&s, "dev", addr(53)));
    }

    #[test]
    fn matches_false_on_addr_mismatch() {
        let s = compose("test", addr(53));
        assert!(!matches(&s, "test", addr(5353)));
    }

    #[test]
    fn parse_handles_domains_without_tilde_prefix() {
        let text = "[Resolve]\nDNS=127.0.0.1:53\nDomains=test\n";
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.domain, "test");
    }
}
