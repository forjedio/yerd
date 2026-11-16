//! `cloudflared` edge-health probe against its metrics server's `/ready`.
//!
//! `cloudflared` answers `GET /ready` with `200` while at least one edge
//! connection is registered and `503` otherwise, so the probe reports real
//! public reachability rather than mere process existence. The metrics address
//! is chosen by the manager per spawn and handed to the child through the
//! `TUNNEL_METRICS` env var. [`is_ok_status_line`] is the pure part; the probe
//! itself is the async I/O edge.

use std::io;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use yerd_supervise::{HealthProbe, Listen};

/// Total bound on one probe (connect, request, and reading the status line), so
/// a wedged metrics server can never stall the supervising driver.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Cap on the bytes read while waiting for the status line.
const STATUS_LINE_LIMIT: u64 = 1024;

/// Whether an HTTP status line reports `200`, e.g. `HTTP/1.1 200 OK`.
#[must_use]
pub fn is_ok_status_line(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let version_ok = parts.next().is_some_and(|v| v.starts_with("HTTP/1."));
    version_ok && parts.next() == Some("200")
}

/// The production `/ready` probe. `Ok(())` means the edge is connected.
#[derive(Debug, Clone, Copy, Default)]
pub struct CloudflaredReadyProbe;

#[async_trait]
impl HealthProbe for CloudflaredReadyProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let Listen::TcpLoopback(addr) = listen else {
            return Err(io::Error::other(
                "cloudflared ready probe requires a TCP address",
            ));
        };
        let addr = *addr;
        let status = tokio::time::timeout(PROBE_TIMEOUT, read_status_line(addr))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ready probe timed out"))??;
        if is_ok_status_line(&status) {
            Ok(())
        } else {
            Err(io::Error::other(format!("cloudflared not ready: {status}")))
        }
    }
}

/// Send `GET /ready` and return the response's first line.
async fn read_status_line(addr: std::net::SocketAddr) -> Result<String, io::Error> {
    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(b"GET /ready HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await?;
    let mut buf = Vec::new();
    let mut limited = stream.take(STATUS_LINE_LIMIT);
    let mut chunk = [0u8; 256];
    loop {
        let n = limited.read(&mut chunk).await?;
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        if n == 0 || buf.contains(&b'\n') {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    Ok(text.lines().next().unwrap_or_default().trim().to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::TcpListener;

    #[test]
    fn status_line_table() {
        for (line, ok) in [
            ("HTTP/1.1 200 OK", true),
            ("HTTP/1.0 200 OK", true),
            ("HTTP/1.1 200", true),
            ("HTTP/1.1 503 Service Unavailable", false),
            ("HTTP/1.1 2000 OK", false),
            ("HTTP/2 200", false),
            ("garbage", false),
            ("", false),
        ] {
            assert_eq!(is_ok_status_line(line), ok, "{line:?}");
        }
    }

    async fn answer_once(listener: &TcpListener, status: &str) {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = [0u8; 512];
        let _ = sock.read(&mut req).await.unwrap();
        let reply = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
        sock.write_all(reply.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn probe_reports_ok_then_not_ready() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let listen = Listen::TcpLoopback(listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            answer_once(&listener, "200 OK").await;
            answer_once(&listener, "503 Service Unavailable").await;
        });

        assert!(CloudflaredReadyProbe.probe(&listen).await.is_ok());
        assert!(CloudflaredReadyProbe.probe(&listen).await.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn probe_fails_when_nothing_listens() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let dead: SocketAddr = listener.local_addr().unwrap();
        drop(listener);
        assert!(CloudflaredReadyProbe
            .probe(&Listen::TcpLoopback(dead))
            .await
            .is_err());
    }
}
