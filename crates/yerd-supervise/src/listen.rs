//! Generic listen-address abstraction.
//!
//! A supervised daemon listens on either a Unix domain socket (Unix) or a TCP
//! loopback address (Windows, and acceptable elsewhere). This is the
//! program-agnostic address type; how a particular consumer *plans* an address
//! (FPM socket naming, a fixed DB port, …) lives in that consumer's crate.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

/// The address a supervised process listens on.
///
/// Wire-level: a Unix path written into `listen = /path/to.sock`, or a
/// `127.0.0.1:<port>` literal written into `listen = 127.0.0.1:9000`.
///
/// A closed set (a socket path or a TCP address) matched exhaustively by
/// consumers in other crates, so it is intentionally not `#[non_exhaustive]`:
/// adding a variant is a deliberate breaking change that should light up every
/// match site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    /// Unix domain socket path. Only valid on Unix.
    UnixSocket(PathBuf),
    /// TCP loopback address. Always valid; required on Windows.
    TcpLoopback(SocketAddr),
}

impl fmt::Display for Listen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnixSocket(p) => f.write_str(&p.display().to_string()),
            Self::TcpLoopback(a) => write!(f, "{a}"),
        }
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
    fn display_unix_socket() {
        let l = Listen::UnixSocket(PathBuf::from("/tmp/fpm-8.3-1234.sock"));
        assert_eq!(l.to_string(), "/tmp/fpm-8.3-1234.sock");
    }

    #[test]
    fn display_tcp_loopback() {
        let l = Listen::TcpLoopback("127.0.0.1:9000".parse().unwrap());
        assert_eq!(l.to_string(), "127.0.0.1:9000");
    }
}
