//! Error types for `yerd-dns`.
//!
//! [`DnsError`] is the single error type exposed by every fallible public API
//! in this crate. Unlike `yerd-tls`'s `TlsError`, it is **not** `Clone + Eq`
//! because it carries `std::io::Error` and `hickory_proto::ProtoError` via
//! `#[source]` - this mirrors `yerd-config::ConfigError`'s pattern. The daemon
//! either logs the error or maps it to `yerd_ipc::ErrorCode` at the IPC
//! boundary.

use std::net::SocketAddr;

use thiserror::Error;

/// Errors produced by `yerd-dns`'s I/O surface.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DnsError {
    /// Could not bind UDP or TCP on `addr`.
    #[error("bind {proto} {addr}: {source}")]
    Bind {
        /// Which transport failed.
        proto: BindProto,
        /// The address the caller asked us to bind.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// UDP bound at the kernel-assigned port, but TCP could not bind to the
    /// same port. Only emitted on the ephemeral path (`addr.port() == 0`)
    /// after [`crate::RETRY_BUDGET`] attempts.
    #[error(
        "port pair mismatch: udp bound at {udp_addr}, tcp bind failed after {attempts} attempts: {source}"
    )]
    PortPairMismatch {
        /// The UDP address actually bound on the final attempt.
        udp_addr: SocketAddr,
        /// How many attempts were made (always equals [`crate::RETRY_BUDGET`]).
        attempts: usize,
        /// Underlying OS error from the final TCP bind failure.
        #[source]
        source: std::io::Error,
    },

    /// `hickory_server::ServerFuture` task returned an error.
    ///
    /// `ProtoError` is `#[non_exhaustive]` upstream, so we carry it directly;
    /// the daemon logs via [`std::fmt::Display`] or walks [`std::error::Error::source`]
    /// to inspect the chain.
    #[error("hickory server task failed: {source}")]
    ServerTask {
        /// Underlying hickory error.
        #[source]
        source: hickory_proto::error::ProtoError,
    },
}

/// Transport tag used in [`DnsError::Bind`].
///
/// Not `#[non_exhaustive]` - this crate services only unencrypted UDP/TCP.
/// `DoT` / `DoH` / `DoQ` would be separate crates entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindProto {
    /// UDP transport.
    Udp,
    /// TCP transport.
    Tcp,
}

impl std::fmt::Display for BindProto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Udp => "UDP",
            Self::Tcp => "TCP",
        })
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
    fn bind_proto_display() {
        assert_eq!(BindProto::Udp.to_string(), "UDP");
        assert_eq!(BindProto::Tcp.to_string(), "TCP");
    }

    #[test]
    fn dns_error_display_pins_per_variant() {
        let addr: SocketAddr = "127.0.0.1:5300".parse().unwrap();
        let io = || std::io::Error::from(std::io::ErrorKind::AddrInUse);

        let bind_udp = DnsError::Bind {
            proto: BindProto::Udp,
            addr,
            source: io(),
        };
        let s = bind_udp.to_string();
        assert!(s.contains("UDP"), "got: {s}");
        assert!(s.contains("127.0.0.1:5300"), "got: {s}");

        let bind_tcp = DnsError::Bind {
            proto: BindProto::Tcp,
            addr,
            source: io(),
        };
        assert!(bind_tcp.to_string().contains("TCP"));

        let mismatch = DnsError::PortPairMismatch {
            udp_addr: addr,
            attempts: crate::RETRY_BUDGET,
            source: io(),
        };
        let s = mismatch.to_string();
        assert!(s.contains("port pair mismatch"), "got: {s}");
        assert!(
            s.contains(&crate::RETRY_BUDGET.to_string()),
            "missing attempt count: {s}"
        );

        let proto = hickory_proto::error::ProtoError::from(io());
        let task = DnsError::ServerTask { source: proto };
        assert!(task.to_string().contains("hickory server task failed"));
    }
}
