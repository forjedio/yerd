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
    /// The daemon accepted the connection but closed it without answering -
    /// it crashed mid-request, or the response exceeded
    /// [`yerd_ipc::DEFAULT_MAX_FRAME`] and could not be written. Split from
    /// [`ClientError::DaemonUnreachable`] so `yerd mcp` can tell an agent which
    /// of the two happened; every other caller treats the pair alike (see
    /// [`ClientError::is_daemon_down`]), so the CLI's exit codes are unchanged.
    #[error("daemon not running — start `yerdd` ({0})")]
    ConnectionClosed(String),
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
    /// Distinct from `Usage` - the invocation was well-formed; the helper said no.
    #[error("{0}")]
    Refused(String),
}

impl ClientError {
    /// Whether this is "the daemon isn't there", covering both a refused
    /// connect and a connection dropped mid-exchange.
    ///
    /// The CLI renders both identically and exits 69, so this is the predicate
    /// its dispatch arms match on: a bare `DaemonUnreachable(_)` pattern would
    /// silently let [`ClientError::ConnectionClosed`] fall through to the
    /// generic error arm (exit 74). `ClientError` is `#[non_exhaustive]` with a
    /// catch-all at every call site, so the compiler cannot catch that for us.
    pub fn is_daemon_down(&self) -> bool {
        matches!(self, Self::DaemonUnreachable(_) | Self::ConnectionClosed(_))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Every `run` dispatch arm keys its exit-69 and doctor rendering on this
    /// predicate. `ClientError` is `#[non_exhaustive]` and each of those matches
    /// ends in a catch-all, so a variant missing here silently downgrades to the
    /// generic exit-74 path instead of failing to compile.
    #[test]
    fn both_daemon_down_shapes_are_recognised() {
        assert!(ClientError::DaemonUnreachable("refused".into()).is_daemon_down());
        assert!(ClientError::ConnectionClosed("closed mid-exchange".into()).is_daemon_down());

        assert!(!ClientError::Usage("bad flag".into()).is_daemon_down());
        assert!(!ClientError::Refused("helper said no".into()).is_daemon_down());
        assert!(
            !ClientError::Ipc(yerd_ipc::IpcError::UnexpectedEof { bytes: 3 }).is_daemon_down(),
            "a framing fault is a transport error (74), not a missing daemon"
        );
    }

    /// The two share an exit code and a rendering; only `yerd mcp` tells them
    /// apart, so their Display text must stay interchangeable to users.
    #[test]
    fn connection_closed_reads_the_same_as_an_absent_daemon() {
        let closed = ClientError::ConnectionClosed("daemon closed the connection".into());
        assert!(
            closed.to_string().contains("daemon not running"),
            "got: {closed}"
        );
    }
}
