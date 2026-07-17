//! `LanIpProvider` - discover the host's own LAN IPv4.
//!
//! LAN mode needs the machine's routable IPv4 as *data* (the `.test` DNS answer
//! served to other devices, the macOS `pf rdr` target, and the bootstrap URL) -
//! never as a bind address (the listeners bind `0.0.0.0`). Reading it is I/O, so
//! it sits behind this trait at the platform edge with a real impl and a fake.
//!
//! [`ActiveLanIpProvider`] uses the UDP-connect trick: bind an unconnected UDP
//! socket, `connect` it toward a TEST-NET-1 address (RFC 5737 - `connect` on a
//! datagram socket sends no packet, it only sets the default peer), then read
//! back `local_addr()`. The kernel fills in the source address it *would* route
//! from, i.e. the default-route interface's IPv4 - the correct answer for the
//! common single-LAN case. It is identical on macOS and Linux and needs no
//! interface-enumeration crate.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use crate::PlatformError;

/// Discover the host's own LAN IPv4 address.
pub trait LanIpProvider {
    /// The host's routable LAN IPv4 (the default-route interface's address).
    ///
    /// Returns [`PlatformError::LanIpDiscovery`] when no address can be
    /// determined (e.g. no route). LAN mode treats that as fail-closed.
    fn lan_ipv4(&self) -> Result<Ipv4Addr, PlatformError>;
}

/// Real [`LanIpProvider`] using the UDP-connect trick.
#[derive(Debug, Default, Clone, Copy)]
pub struct ActiveLanIpProvider;

impl ActiveLanIpProvider {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// TEST-NET-1 (RFC 5737) - a documentation address that is never routed. Used
/// only as the `connect` target so the kernel selects a source interface; no
/// datagram is ever sent.
const PROBE_TARGET: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 1);

impl LanIpProvider for ActiveLanIpProvider {
    fn lan_ipv4(&self) -> Result<Ipv4Addr, PlatformError> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .map_err(|source| PlatformError::LanIpDiscovery { source })?;
        socket
            .connect(PROBE_TARGET)
            .map_err(|source| PlatformError::LanIpDiscovery { source })?;
        let local = socket
            .local_addr()
            .map_err(|source| PlatformError::LanIpDiscovery { source })?;
        match local.ip() {
            std::net::IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Ok(v4),
            _ => Err(PlatformError::LanIpDiscovery {
                source: std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "no routable LAN IPv4",
                ),
            }),
        }
    }
}

/// Test fake returning a fixed IPv4 (or a discovery error).
#[derive(Debug, Clone, Copy)]
pub struct FakeLanIpProvider {
    /// The address to return; `None` yields a [`PlatformError::LanIpDiscovery`].
    pub ip: Option<Ipv4Addr>,
}

impl FakeLanIpProvider {
    /// A fake that always returns `ip`.
    #[must_use]
    pub const fn new(ip: Ipv4Addr) -> Self {
        Self { ip: Some(ip) }
    }

    /// A fake whose discovery always fails.
    #[must_use]
    pub const fn failing() -> Self {
        Self { ip: None }
    }
}

impl LanIpProvider for FakeLanIpProvider {
    fn lan_ipv4(&self) -> Result<Ipv4Addr, PlatformError> {
        self.ip.ok_or_else(|| PlatformError::LanIpDiscovery {
            source: std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "fake failure"),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn fake_returns_configured_ip() {
        let p = FakeLanIpProvider::new(Ipv4Addr::new(192, 168, 1, 42));
        assert_eq!(p.lan_ipv4().unwrap(), Ipv4Addr::new(192, 168, 1, 42));
    }

    #[test]
    fn fake_failing_yields_discovery_error() {
        let p = FakeLanIpProvider::failing();
        assert!(matches!(
            p.lan_ipv4(),
            Err(PlatformError::LanIpDiscovery { .. })
        ));
    }

    #[test]
    fn active_returns_non_loopback_or_fails_closed() {
        // On a CI host with no route this errors; on a networked host it yields a
        // routable IPv4. Both are acceptable - we only assert it never returns
        // loopback/unspecified as a "success".
        match ActiveLanIpProvider::new().lan_ipv4() {
            Ok(ip) => {
                assert!(!ip.is_loopback());
                assert!(!ip.is_unspecified());
            }
            Err(PlatformError::LanIpDiscovery { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
