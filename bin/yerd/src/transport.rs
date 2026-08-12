//! Daemon connection + one-shot request/response exchange.
//!
//! The socket path is derived **identically to the daemon**
//! (`<runtime>/yerd.sock`, where `<runtime>` comes from
//! `yerd_platform::Paths::resolve`) so client and server always agree,
//! including the `/tmp/yerd-$UID` fallback when `XDG_RUNTIME_DIR` is unset.

use crate::error::ClientError;
use yerd_ipc::{Request, Response};

/// Resolve the daemon socket path and exchange one request/response.
///
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
    use interprocess::local_socket::tokio::Stream as IpcStream;
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use yerd_ipc::{read_message, write_message, FrameDecoder, DEFAULT_MAX_FRAME};

    let name = sock
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| ClientError::DaemonUnreachable(format!("{}: {e}", sock.display())))?;
    let stream = IpcStream::connect(name)
        .await
        .map_err(|e| ClientError::DaemonUnreachable(format!("{}: {e}", sock.display())))?;

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

#[cfg(windows)]
/// Exchange one request over the Windows named pipe.
pub async fn exchange(req: &Request) -> Result<Response, ClientError> {
    use interprocess::local_socket::tokio::Stream as IpcStream;
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    use yerd_ipc::{read_message, write_message, FrameDecoder, DEFAULT_MAX_FRAME};
    use yerd_platform::{ActivePaths, Paths};

    let dirs = ActivePaths::new().resolve()?;
    let pipe = yerd_platform::paths::daemon_pipe_name(&dirs);
    let name = pipe
        .as_str()
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| ClientError::DaemonUnreachable(format!("{pipe}: {e}")))?;
    let stream = IpcStream::connect(name)
        .await
        .map_err(|e| ClientError::DaemonUnreachable(format!("{pipe}: {e}")))?;
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
