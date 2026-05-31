//! Protocol-level readiness probes.
//!
//! The probe is the signal that ends the supervisor's `Starting` window, so it
//! must confirm the server actually answers its protocol — not merely that the
//! TCP port is open. Phase 1 ships the Redis probe (`PING` → `+PONG`);
//! MySQL/Postgres handshake probes land in Phase 2.

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use yerd_supervise::{HealthProbe, Listen};

/// Redis/Valkey readiness probe: open a TCP connection, send an inline `PING`,
/// and require a `+PONG` reply.
pub struct RedisProbe;

#[async_trait]
impl HealthProbe for RedisProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let addr = match listen {
            Listen::TcpLoopback(a) => *a,
            Listen::UnixSocket(_) => {
                return Err(io::Error::other(
                    "redis probe requires a TCP listen address",
                ))
            }
        };
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(b"PING\r\n").await?;
        let mut buf = [0u8; 16];
        let n = stream.read(&mut buf).await?;
        let reply = buf.get(..n).unwrap_or(&[]);
        // A healthy server answers RESP `+PONG\r\n`. Accept any case of "pong".
        if reply.starts_with(b"+PONG") || reply.eq_ignore_ascii_case(b"+pong\r\n") {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to PING",
            ))
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
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::TcpListener;

    /// Spin a one-shot fake that replies `+PONG` and assert the probe is happy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probe_accepts_pong() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 16];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"+PONG\r\n").await;
            }
        });
        let probe = RedisProbe;
        probe.probe(&Listen::TcpLoopback(addr)).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probe_rejects_garbage() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 16];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"-ERR nope\r\n").await;
            }
        });
        let probe = RedisProbe;
        assert!(probe.probe(&Listen::TcpLoopback(addr)).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probe_fails_when_nothing_listening() {
        let probe = RedisProbe;
        let dead: SocketAddr = (Ipv4Addr::LOCALHOST, 1).into();
        assert!(probe.probe(&Listen::TcpLoopback(dead)).await.is_err());
    }
}
