//! The `Backend` enum - where a routed request gets forwarded to.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Per-site forwarding target.
///
/// `From<yerd_php::Listen>` is intentionally **not** implemented - the
/// daemon's [`crate::traits::BackendResolver`] impl translates between the
/// two so `yerd-proxy` doesn't depend on `yerd-php`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// FastCGI over a Unix domain socket. Unix-only.
    PhpFpm {
        /// Path to the FPM-listening socket.
        socket: PathBuf,
    },
    /// FastCGI over TCP loopback. Required on Windows; allowed elsewhere.
    PhpFpmTcp {
        /// TCP address of the FPM listener.
        addr: SocketAddr,
    },
    /// Plain HTTP/1.1 to a FrankenPHP worker.
    FrankenPhp {
        /// TCP address of the FrankenPHP worker.
        addr: SocketAddr,
    },
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PhpFpm { socket } => write!(f, "fpm-unix:{}", socket.display()),
            Self::PhpFpmTcp { addr } => write!(f, "fpm-tcp:{addr}"),
            Self::FrankenPhp { addr } => write!(f, "franken:{addr}"),
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
    fn display_per_variant() {
        let unix = Backend::PhpFpm {
            socket: PathBuf::from("/run/fpm.sock"),
        };
        assert_eq!(unix.to_string(), "fpm-unix:/run/fpm.sock");

        let tcp = Backend::PhpFpmTcp {
            addr: "127.0.0.1:9000".parse().unwrap(),
        };
        assert_eq!(tcp.to_string(), "fpm-tcp:127.0.0.1:9000");

        let franken = Backend::FrankenPhp {
            addr: "127.0.0.1:8080".parse().unwrap(),
        };
        assert_eq!(franken.to_string(), "franken:127.0.0.1:8080");
    }
}
