//! Daemon connection + one-shot request/response exchange.
//!
//! The client derives the transport address **identically to the daemon** so
//! the two always agree:
//! - Unix: `<runtime>/yerd.sock`, where `<runtime>` comes from
//!   `yerd_platform::Paths::resolve` (including the `/tmp/yerd-$UID` fallback
//!   when `XDG_RUNTIME_DIR` is unset).
//! - Windows: the named pipe `yerd_platform::daemon_pipe_name(&dirs)` derives
//!   from the current user's SID and `dirs.runtime` (see
//!   `yerd_platform::pure::win_pipe`).

use crate::error::ClientError;
use interprocess::local_socket::tokio::Stream as IpcStream;
use yerd_ipc::{Request, Response};

/// Resolve the daemon socket path and exchange one request/response.
#[cfg(unix)]
pub async fn exchange(req: &Request) -> Result<Response, ClientError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new().resolve()?;
    exchange_at(&dirs.runtime.join("yerd.sock"), req).await
}

/// Connect to the daemon at an explicit socket path and exchange one
/// request/response. Factored out of [`exchange`] so integration tests can
/// target a tempdir socket. Unix only.
#[cfg(unix)]
pub async fn exchange_at(sock: &std::path::Path, req: &Request) -> Result<Response, ClientError> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericFilePath, ToFsName};

    let name = sock
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| ClientError::DaemonUnreachable(format!("{}: {e}", sock.display())))?;
    let stream = IpcStream::connect(name)
        .await
        .map_err(|e| ClientError::DaemonUnreachable(format!("{}: {e}", sock.display())))?;
    post_connect_exchange(stream, req).await
}

/// Resolve the daemon pipe name and exchange one request/response (Windows).
#[cfg(windows)]
pub async fn exchange(req: &Request) -> Result<Response, ClientError> {
    use yerd_platform::{ActivePaths, Paths};
    let dirs = ActivePaths::new().resolve()?;
    let name = yerd_platform::daemon_pipe_name(&dirs)?;
    exchange_at_name(&name, req).await
}

/// Connect to the daemon at an explicit named pipe and exchange one
/// request/response. Mirror of [`exchange_at`] for the Windows namespace, so
/// integration tests can target a tempdir-derived pipe.
#[cfg(windows)]
pub async fn exchange_at_name(name: &str, req: &Request) -> Result<Response, ClientError> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericNamespaced, ToNsName};

    let ns = name
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| ClientError::DaemonUnreachable(format!("{name}: {e}")))?;
    let stream = IpcStream::connect(ns)
        .await
        .map_err(|e| ClientError::DaemonUnreachable(format!("{name}: {e}")))?;
    post_connect_exchange(stream, req).await
}

/// The transport-agnostic half of an exchange: split the connected stream,
/// write the request frame, read one response frame. Shared by every cfg arm so
/// the framing never diverges between Unix and Windows.
async fn post_connect_exchange(stream: IpcStream, req: &Request) -> Result<Response, ClientError> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use yerd_ipc::{read_message, write_message, FrameDecoder, DEFAULT_MAX_FRAME};

    let (reader, writer) = stream.split();
    let mut reader = reader;
    let mut writer = writer;
    write_message(&mut writer, req, DEFAULT_MAX_FRAME).await?;
    let mut decoder = FrameDecoder::new();
    match read_message::<_, Response>(&mut reader, &mut decoder).await? {
        Some(resp) => Ok(resp),
        None => Err(ClientError::ConnectionClosed(
            "daemon closed the connection without responding".to_owned(),
        )),
    }
}
