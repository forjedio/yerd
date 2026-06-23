//! CLI client error type.

/// Errors the CLI can produce while mapping a command or talking to the daemon.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// A command argument failed client-side validation (bad name / version).
    /// Surfaces as a usage error (exit 2) before any socket connect.
    #[error("{0}")]
    Usage(String),
    /// The daemon socket could not be reached (not running, or refused).
    #[error("daemon not running — start `yerdd` ({0})")]
    DaemonUnreachable(String),
    /// IPC framing/codec error talking to the daemon.
    #[error("ipc: {0}")]
    Ipc(#[from] yerd_ipc::IpcError),
    /// Resolving the runtime/socket directory failed.
    #[error("platform: {0}")]
    Platform(#[from] yerd_platform::PlatformError),
    /// The daemon reported a malformed CA fingerprint (used by `elevate`).
    #[error("{0}")]
    Fingerprint(#[from] yerd_platform::FingerprintParseError),
    /// `yerd-helper` declined a privileged operation for a safety reason (e.g.
    /// refused to remove a trust-store cert it couldn't confirm is yerd's).
    /// Distinct from `Usage` — the invocation was well-formed; the helper said no.
    #[error("{0}")]
    Refused(String),
}
