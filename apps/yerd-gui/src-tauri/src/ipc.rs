//! The daemon transport - a near-verbatim mirror of `bin/yerd/src/transport.rs`.
//!
//! The socket path is derived identically to the daemon and the CLI
//! (`<runtime>/yerd.sock`, where `<runtime>` comes from
//! `yerd_platform::Paths::resolve`) so client and server always agree. This is
//! the thin-client rule in practice: the bridge owns the transport, nothing else.

use crate::error::GuiError;
use yerd_ipc::{Request, Response};

/// Resolve the daemon socket and exchange one request/response.
#[cfg(unix)]
pub async fn exchange(req: &Request) -> Result<Response, GuiError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| GuiError::unreachable(format!("cannot resolve runtime dir: {e}")))?;
    exchange_at(&dirs.runtime.join("yerd.sock"), req).await
}

/// [`exchange`] bounded by a timeout. Used by the liveness/probe commands
/// (`status`/`ping`/`daemon_info`) so a daemon that accepts the socket but never
/// replies (crash-looping, wedged, mid-startup) can't make the `invoke` promise
/// hang forever - the unbounded `read_message` in [`exchange_at`] otherwise never
/// resolves, freezing the start spinner and the poller. On elapse this returns an
/// `unreachable` error so the poller treats it as "Stopped" and the start flow
/// advances to its diagnostics ceiling. NOT used for long-running ops (installs/
/// updates legitimately block for minutes).
pub async fn exchange_timeout(
    req: &Request,
    timeout: std::time::Duration,
) -> Result<Response, GuiError> {
    match tokio::time::timeout(timeout, exchange(req)).await {
        Ok(res) => res,
        Err(_) => Err(GuiError::unreachable(format!(
            "daemon did not respond within {}s (it may be starting, wedged, or crash-looping)",
            timeout.as_secs()
        ))),
    }
}

/// Connect at an explicit socket path and exchange one request/response.
/// Factored out so tests can target a tempdir socket. Unix only.
#[cfg(unix)]
pub async fn exchange_at(sock: &std::path::Path, req: &Request) -> Result<Response, GuiError> {
    use interprocess::local_socket::tokio::Stream as IpcStream;
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use yerd_ipc::{read_message, write_message, FrameDecoder, DEFAULT_MAX_FRAME};

    let name = sock
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| GuiError::unreachable(format!("{}: {e}", sock.display())))?;
    let stream = IpcStream::connect(name)
        .await
        .map_err(|e| GuiError::unreachable(format!("{}: {e}", sock.display())))?;

    let (reader, writer) = stream.split();
    let mut reader = reader;
    let mut writer = writer;
    write_message(&mut writer, req, DEFAULT_MAX_FRAME)
        .await
        .map_err(|e| GuiError::internal(format!("write: {e}")))?;
    let mut decoder = FrameDecoder::new();
    match read_message::<_, Response>(&mut reader, &mut decoder)
        .await
        .map_err(|e| GuiError::internal(format!("read: {e}")))?
    {
        Some(resp) => Ok(resp),
        None => Err(GuiError::unreachable(
            "daemon closed the connection without responding",
        )),
    }
}

/// The Windows named-pipe name is non-deterministic for clients today (a tracked
/// Phase-2 follow-up), so the GUI is macOS/Linux-only for now - exactly as the
/// CLI's transport is.
#[cfg(not(unix))]
pub async fn exchange(_req: &Request) -> Result<Response, GuiError> {
    Err(GuiError::unreachable(
        "the Windows IPC client is not yet supported (daemon pipe name is non-deterministic)",
    ))
}
