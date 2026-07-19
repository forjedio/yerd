//! Compose and match Yerd-owned `NetworkManager` dnsmasq configuration.

use std::net::{IpAddr, SocketAddr};

/// Enable `NetworkManager`'s supported dnsmasq DNS plugin.
#[must_use]
pub fn compose_networkmanager() -> String {
    "# Managed by Yerd.\n[main]\ndns=dnsmasq\n".to_owned()
}

/// Whether a `NetworkManager` snippet selects dnsmasq in its `[main]` section.
#[must_use]
pub fn matches_networkmanager(text: &str) -> bool {
    let mut in_main = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_main = section.eq_ignore_ascii_case("main");
            continue;
        }
        if in_main {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("dns") {
                    return value.trim().eq_ignore_ascii_case("dnsmasq");
                }
            }
        }
    }
    false
}

/// Compose a dnsmasq route for exactly one domain suffix.
#[must_use]
pub fn compose_dnsmasq(tld: &str, addr: SocketAddr) -> String {
    format!(
        "# Managed by Yerd.\nserver=/{tld}/{}#{}\n",
        addr.ip(),
        addr.port()
    )
}

/// Whether a dnsmasq snippet contains Yerd's exact domain, address and port.
#[must_use]
pub fn matches_dnsmasq(text: &str, tld: &str, addr: SocketAddr) -> bool {
    text.lines().map(str::trim).any(|line| {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            return false;
        }
        let Some(value) = line.strip_prefix("server=/") else {
            return false;
        };
        let Some((domain, server)) = value.split_once('/') else {
            return false;
        };
        let Some((ip, port)) = server.rsplit_once('#') else {
            return false;
        };
        domain == tld
            && ip.parse::<IpAddr>() == Ok(addr.ip())
            && port.parse::<u16>() == Ok(addr.port())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn addr() -> SocketAddr {
        "127.0.0.1:1053".parse().unwrap()
    }

    #[test]
    fn networkmanager_round_trip_and_comments() {
        assert!(matches_networkmanager(&compose_networkmanager()));
        assert!(matches_networkmanager(
            "# note\n[main]\nfoo=bar\ndns = dnsmasq\n"
        ));
        assert!(!matches_networkmanager("[main]\ndns=systemd-resolved\n"));
        assert!(!matches_networkmanager("dns=dnsmasq\n"));
    }

    #[test]
    fn dnsmasq_round_trip_and_comments() {
        assert_eq!(
            compose_dnsmasq("test", addr()),
            "# Managed by Yerd.\nserver=/test/127.0.0.1#1053\n"
        );
        assert!(matches_dnsmasq(
            "# note\nserver=/test/127.0.0.1#1053\n",
            "test",
            addr()
        ));
    }

    #[test]
    fn dnsmasq_rejects_malformed_or_stale_values() {
        assert!(!matches_dnsmasq("server=/test/127.0.0.1\n", "test", addr()));
        assert!(!matches_dnsmasq(
            "server=/dev/127.0.0.1#1053\n",
            "test",
            addr()
        ));
        assert!(!matches_dnsmasq(
            "server=/test/127.0.0.2#1053\n",
            "test",
            addr()
        ));
        assert!(!matches_dnsmasq(
            "server=/test/127.0.0.1#5353\n",
            "test",
            addr()
        ));
    }

    #[test]
    fn dnsmasq_matches_equivalent_ipv6_spellings() {
        let addr: SocketAddr = "[2001:db8::1]:1053".parse().unwrap();
        assert!(matches_dnsmasq(
            "server=/test/2001:0db8:0:0:0:0:0:1#1053\n",
            "test",
            addr
        ));
    }
}
