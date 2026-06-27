//! Daemon-side error type + sysexits-style exit codes.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the daemon during startup or steady-state.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// Config load/parse/validate failure.
    #[error("config: {0}")]
    Config(#[from] yerd_config::ConfigError),
    /// Failure constructing a domain type (e.g. invalid TLD/site).
    #[error("core: {0}")]
    Core(#[from] yerd_core::CoreError),
    /// Platform operation (path resolution, port binding, etc.) failed.
    #[error("platform: {0}")]
    Platform(#[from] yerd_platform::PlatformError),
    /// CA / leaf certificate operation failed.
    #[error("tls: {0}")]
    Tls(#[from] yerd_tls::TlsError),
    /// DNS server bind/serve failure.
    #[error("dns: {0}")]
    Dns(#[from] yerd_dns::DnsError),
    /// Proxy server failure.
    #[error("proxy: {0}")]
    Proxy(#[from] yerd_proxy::ProxyError),
    /// PHP-FPM supervisor failure.
    #[error("php: {0}")]
    Php(#[from] yerd_php::PhpError),
    /// IPC codec failure.
    #[error("ipc: {0}")]
    Ipc(#[from] yerd_ipc::IpcError),
    /// Another `yerdd` process holds the single-instance lock.
    #[error("another yerdd is already running (lock held at {})", path.display())]
    AlreadyRunning {
        /// Path the lock file lives at.
        path: PathBuf,
    },
    /// Generic filesystem I/O failure.
    #[error("io at {}: {source}", path.display())]
    Io {
        /// The path involved in the failed operation.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },
}

/// Sysexits-style exit code for the given error.
///
/// 70  EX_SOFTWARE  generic software failure (fallback)
/// 71  EX_OSERR     OS error (platform / TLS)
/// 74  EX_IOERR     I/O error
/// 75  EX_TEMPFAIL  temporary failure - another instance already running
/// 78  EX_CONFIG    config error (Config / Core)
#[must_use]
pub fn exit_code(e: &DaemonError) -> u8 {
    match e {
        DaemonError::AlreadyRunning { .. } => 75,
        DaemonError::Config(_) | DaemonError::Core(_) => 78,
        DaemonError::Io { .. } => 74,
        DaemonError::Platform(_) | DaemonError::Tls(_) => 71,
        _ => 70,
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

    fn io_err() -> io::Error {
        io::Error::from(io::ErrorKind::AddrInUse)
    }

    #[test]
    fn exit_codes_pinned() {
        assert_eq!(
            exit_code(&DaemonError::AlreadyRunning {
                path: PathBuf::from("/tmp/x")
            }),
            75
        );
        assert_eq!(
            exit_code(&DaemonError::Io {
                path: PathBuf::from("/tmp/x"),
                source: io_err(),
            }),
            74
        );
        let cfg_err: yerd_config::ConfigError = yerd_config::ConfigError::Io {
            path: PathBuf::from("/tmp/x"),
            source: io_err(),
        };
        assert_eq!(exit_code(&DaemonError::from(cfg_err)), 78);
    }

    #[test]
    fn already_running_display_includes_path() {
        let e = DaemonError::AlreadyRunning {
            path: PathBuf::from("/tmp/yerd.lock"),
        };
        let s = e.to_string();
        assert!(s.contains("already running"));
        assert!(s.contains("/tmp/yerd.lock"));
    }
}
