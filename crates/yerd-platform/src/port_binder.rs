//! `PortBinder` trait + result types.

use std::net::TcpListener;

use crate::PlatformError;

/// A bound TCP listener returned by [`PortBinder::bind`] and the two
/// halves of [`PortPair`].
///
/// `TcpListener` is intentionally `std::net::TcpListener` - not the tokio
/// variant - so `yerd-platform` does not pull `tokio` into its public
/// surface. `yerd-proxy` (which does use tokio) can convert via
/// `tokio::net::TcpListener::from_std`.
#[derive(Debug)]
pub struct BoundPort {
    /// The underlying listener.
    pub listener: TcpListener,
}

impl BoundPort {
    /// Resolved local port. Sourced from
    /// [`TcpListener::local_addr`] so it is correct even when the
    /// original bind argument was `0`.
    pub fn port(&self) -> std::io::Result<u16> {
        Ok(self.listener.local_addr()?.port())
    }
}

/// The HTTP + HTTPS pair returned by [`PortBinder::bind_pair`].
#[derive(Debug)]
pub struct PortPair {
    /// HTTP listener.
    pub http: BoundPort,
    /// HTTPS listener.
    pub https: BoundPort,
}

/// OS port-binding abstraction.
pub trait PortBinder {
    /// Bind a single TCP listener on `127.0.0.1:port`. Useful for tests
    /// and the DNS port. `io::Error` is mapped to [`PlatformError::Bind`].
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError>;

    /// Bind 80 and 443 (or the rootless equivalent) atomically.
    ///
    /// When `lan` is `false` the listeners bind loopback (`127.0.0.1`); when
    /// `true` they bind the wildcard address (`0.0.0.0`) so the ports are
    /// reachable from other devices on the LAN. Binding `0.0.0.0` still accepts
    /// loopback traffic, so on-host access keeps working. Privilege for a
    /// sub-1024 wildcard bind is the OS's concern (Linux `setcap`; on macOS the
    /// daemon binds the rootless fallback and a `pf` redirect carries 80/443).
    ///
    /// Internally: attempt `desired.0` then `desired.1`. If either fails
    /// with one of the retry-trigger kinds - `PermissionDenied`,
    /// `AddrInUse`, or `AddrNotAvailable` - drop any successful partial
    /// listener and retry with `(fallback.0, fallback.1)`. Any other
    /// `io::Error` on the desired pair surfaces immediately as
    /// [`PlatformError::Bind`] without trying the fallback. If both
    /// pairs fail, the error is
    /// [`PlatformError::BindPair`] carrying all four `ErrorKind`s.
    ///
    /// On macOS a privileged `desired` side is never attempted: the pair is
    /// replaced by `fallback` first, so the daemon deterministically owns the
    /// rootless ports the `pf` redirect targets and never squats 80/443.
    fn bind_pair(
        &self,
        lan: bool,
        desired: (u16, u16),
        fallback: (u16, u16),
    ) -> Result<PortPair, PlatformError>;
}
