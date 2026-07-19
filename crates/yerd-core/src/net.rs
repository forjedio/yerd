//! Pure network-scope predicates shared across the LAN-exposure surfaces.

use std::net::{IpAddr, Ipv4Addr};

/// Whether `ip` belongs to a private/local scope Yerd is willing to serve in LAN
/// mode: RFC 1918 (`10/8`, `172.16/12`, `192.168/16`), link-local (`169.254/16`),
/// or loopback (so the host's own traffic is never rejected). CGNAT (`100.64/10`)
/// is deliberately excluded - it is neither a home/office LAN nor loopback.
///
/// This is a **blast-radius reducer, not authentication**: any device already on
/// the LAN has a qualifying address. It exists so a wildcard (`0.0.0.0`) bind on
/// a multi-homed or internet-facing host does not serve sites to arbitrary
/// public peers. It is the single predicate the proxy accept loops, the DNS
/// handler, and the bootstrap endpoint all share, so their scoping agrees.
///
/// IPv6 LAN exposure is out of scope (Yerd binds no v6 listener today), so only
/// loopback (`::1`) and IPv4-mapped private addresses qualify on the v6 side.
#[must_use]
pub fn is_lan_source(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_lan_source_v4(v4),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(is_lan_source_v4),
    }
}

fn is_lan_source_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_private() || v4.is_link_local()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn accepts_rfc1918_link_local_and_loopback() {
        assert!(is_lan_source(v4(10, 0, 0, 1)));
        assert!(is_lan_source(v4(172, 16, 5, 9)));
        assert!(is_lan_source(v4(192, 168, 1, 42)));
        assert!(is_lan_source(v4(169, 254, 3, 3)));
        assert!(is_lan_source(v4(127, 0, 0, 1)));
        assert!(is_lan_source(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn rejects_public_and_cgnat_100_64_slash_10() {
        assert!(!is_lan_source(v4(8, 8, 8, 8)));
        assert!(!is_lan_source(v4(1, 1, 1, 1)));
        assert!(!is_lan_source(v4(172, 32, 0, 1)));
        assert!(!is_lan_source(v4(100, 64, 0, 1)));
        assert!(!is_lan_source(v4(100, 127, 255, 254)));
    }
}
