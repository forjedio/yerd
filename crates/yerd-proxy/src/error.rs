//! Errors produced by `yerd-proxy`.
//!
//! Not `Clone + Eq` - wraps `io::Error`, `hyper::Error`, `rustls::Error`,
//! and a `Box<dyn Error>` source. The crate mirrors `yerd-config` /
//! `yerd-php`'s shape; the daemon translates to a stable IPC code when
//! crossing the wire.

use std::io;

use thiserror::Error;

use crate::pure::fcgi_codec::FcgiError;

/// Errors produced by `yerd-proxy`'s I/O surface.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProxyError {
    /// A listener `accept()` returned an error.
    #[error("accept failed: {source}")]
    Accept {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// A `BackendResolver` impl returned an error originating outside
    /// this crate (typically `yerd_php::PhpError`). The daemon wraps
    /// its foreign error here so `yerd-proxy` doesn't depend on
    /// `yerd-php`.
    #[error("backend resolver failed for site {host}: {source}")]
    BackendResolver {
        /// Site hostname the resolver was asked about.
        host: String,
        /// Boxed foreign error (typically `PhpError`).
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// Could not open a connection to the backend.
    #[error("connect to backend {backend}: {source}")]
    BackendConnect {
        /// `Backend::Display` of the target.
        backend: String,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Backend protocol I/O failed (FastCGI socket read/write, or upstream HTTP).
    #[error("backend protocol: {source}")]
    BackendProtocol {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Upgrade tunnel failed mid-stream.
    #[error("upgrade tunnel failed: {source}")]
    Upgrade {
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// FastCGI framing error.
    #[error("FastCGI codec error: {source}")]
    Fcgi {
        /// Underlying codec error.
        #[from]
        source: FcgiError,
    },

    /// Hyper error during request/response handling.
    #[error("hyper error: {source}")]
    Hyper {
        /// Underlying hyper error.
        #[source]
        source: hyper::Error,
    },

    /// rustls error during TLS server config build or handshake.
    #[error("TLS error: {source}")]
    Tls {
        /// Underlying rustls error.
        #[source]
        source: rustls::Error,
    },
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

    fn io_err() -> io::Error {
        io::Error::from(io::ErrorKind::ConnectionRefused)
    }

    #[test]
    fn display_pins_per_variant() {
        let e = ProxyError::Accept { source: io_err() };
        assert!(e.to_string().contains("accept failed"));

        let e = ProxyError::BackendResolver {
            host: "app.test".into(),
            source: Box::new(io_err()),
        };
        let s = e.to_string();
        assert!(s.contains("backend resolver failed"));
        assert!(s.contains("app.test"));

        let e = ProxyError::BackendConnect {
            backend: "fpm-unix:/x.sock".into(),
            source: io_err(),
        };
        assert!(e.to_string().contains("fpm-unix:/x.sock"));

        let e = ProxyError::BackendProtocol { source: io_err() };
        assert!(e.to_string().contains("backend protocol"));

        let e = ProxyError::Upgrade { source: io_err() };
        assert!(e.to_string().contains("upgrade tunnel"));

        let e = ProxyError::Fcgi {
            source: FcgiError::BadVersion(7),
        };
        let s = e.to_string();
        assert!(s.contains("FastCGI codec error"));
        assert!(s.contains("version 7"));
    }
}
