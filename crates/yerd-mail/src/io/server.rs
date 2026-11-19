//! The tokio SMTP capture server: an accept loop that drives the pure
//! [`Session`](crate::pure::smtp::Session) per connection and persists each
//! captured message to the [`Store`].

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::error::MailError;
use crate::io::store::Store;
use crate::pure::smtp::{Reply, Session};

/// Maps the host a client announced in `EHLO`/`HELO` to the site it belongs
/// to, so captured mail can be attributed. The daemon backs this with its live
/// site router; the mail crate cannot see sites itself.
pub trait SiteResolver: Send + Sync + 'static {
    /// The name of the site serving `host`, or `None` when no site matches.
    fn site_for_host(&self, host: &str) -> impl Future<Output = Option<String>> + Send;
}

/// Largest message we will buffer (defensive cap against a runaway sender).
const MAX_MESSAGE_BYTES: usize = 25 * 1024 * 1024;

/// Bind the capture SMTP listener on `127.0.0.1:<port>`.
///
/// # Errors
/// [`MailError::Bind`] when the port can't be bound (e.g. already in use). The
/// daemon treats this as non-fatal.
pub async fn bind(port: u16) -> Result<TcpListener, MailError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpListener::bind(addr)
        .await
        .map_err(|source| MailError::Bind { port, source })
}

/// Accept connections and capture mail until `shutdown` resolves. Each
/// captured message is attributed to a site through `resolver`.
///
/// `S: Send + 'static` because the daemon `tokio::spawn`s this future.
pub async fn serve<S, R>(
    listener: TcpListener,
    store: Arc<Store>,
    resolver: Arc<R>,
    shutdown: S,
) -> Result<(), MailError>
where
    S: Future<Output = ()> + Send + 'static,
    R: SiteResolver,
{
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    let store = store.clone();
                    let resolver = resolver.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, store, resolver).await {
                            tracing::debug!(error = %e, "mail connection ended with error");
                        }
                    });
                }
                Err(e) => tracing::debug!(error = %e, "mail accept failed"),
            },
        }
    }
    Ok(())
}

/// Drive one connection through the SMTP exchange, persisting each completed
/// `DATA` payload.
async fn handle_conn<R: SiteResolver>(
    stream: TcpStream,
    store: Arc<Store>,
    resolver: Arc<R>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    write_half.write_all(Session::greeting().as_bytes()).await?;

    let mut session = Session::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        match session.command(&text) {
            Reply::Line(r) => write_half.write_all(r.as_bytes()).await?,
            Reply::Close(r) => {
                write_half.write_all(r.as_bytes()).await?;
                break;
            }
            Reply::StartData(r) => {
                write_half.write_all(r.as_bytes()).await?;
                let data = read_data(&mut reader).await?;
                let msg = session.finish_data(&data);
                let site = match msg.helo.as_deref() {
                    Some(host) => resolver.site_for_host(host).await,
                    None => None,
                };
                match store.append(&msg.raw, site).await {
                    Ok(()) => write_half.write_all(b"250 OK: queued\r\n").await?,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to store captured mail");
                        write_half.write_all(b"451 storage error\r\n").await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Read `DATA` lines until the terminating `\r\n.\r\n` (a line that is just
/// `.`). Returns the body bytes (still dot-stuffed; the session unstuffs them).
async fn read_data<R>(reader: &mut BufReader<R>) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut data = Vec::new();
    let mut line = Vec::new();
    let mut capped = false;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            break;
        }
        if line == b".\r\n" || line == b".\n" {
            break;
        }
        if capped {
            continue;
        }
        if data.len() + line.len() > MAX_MESSAGE_BYTES {
            let take = MAX_MESSAGE_BYTES.saturating_sub(data.len());
            data.extend_from_slice(line.get(..take).unwrap_or(&line));
            capped = true;
            continue;
        }
        data.extend_from_slice(&line);
    }
    Ok(data)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::items_after_statements
)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    struct FakeResolver;

    impl SiteResolver for FakeResolver {
        fn site_for_host(&self, host: &str) -> impl Future<Output = Option<String>> + Send {
            let site = (host == "blog.test").then(|| "blog".to_string());
            async move { site }
        }
    }

    async fn capture_with_helo(helo: &str) -> Vec<yerd_ipc::MailSummary> {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().to_path_buf()).unwrap());
        let listener = bind(0).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let serve_store = store.clone();
        let handle = tokio::spawn(async move {
            serve(listener, serve_store, Arc::new(FakeResolver), async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 256];
        let n = client.read(&mut buf).await.unwrap();
        assert!(buf[..n].starts_with(b"220 "));

        async fn cmd(c: &mut TcpStream, line: &str) {
            c.write_all(line.as_bytes()).await.unwrap();
            let mut b = [0u8; 256];
            let _ = c.read(&mut b).await.unwrap();
        }
        cmd(&mut client, &format!("EHLO {helo}\r\n")).await;
        cmd(&mut client, "MAIL FROM:<a@b.c>\r\n").await;
        cmd(&mut client, "RCPT TO:<d@e.f>\r\n").await;
        cmd(&mut client, "DATA\r\n").await;
        client
            .write_all(b"Subject: Hello\r\n\r\nWorld\r\n.\r\n")
            .await
            .unwrap();
        let mut b = [0u8; 256];
        let n = client.read(&mut b).await.unwrap();
        assert!(b[..n].starts_with(b"250 "), "expected 250 after data");
        cmd(&mut client, "QUIT\r\n").await;

        for _ in 0..50 {
            if store.count().await == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let list = store.list().await;
        let _ = tx.send(());
        let _ = handle.await;
        list
    }

    #[tokio::test]
    async fn captures_a_message_over_tcp() {
        let list = capture_with_helo("test").await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].subject, "Hello");
        assert_eq!(list[0].site, None);
    }

    #[tokio::test]
    async fn attributes_mail_to_the_ehlo_site() {
        let list = capture_with_helo("blog.test").await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].site.as_deref(), Some("blog"));
    }
}
