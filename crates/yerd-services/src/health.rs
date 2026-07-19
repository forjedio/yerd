//! Protocol-level readiness probes.
//!
//! The probe is the signal that ends the supervisor's `Starting` window, so it
//! must confirm the server actually answers its protocol - not merely that the
//! TCP port is open. Redis answers `PING` with `+PONG`; `MySQL`/`MariaDB` send an
//! initial handshake packet; Postgres replies to a startup message.
//!
//! [`ReadinessProbe`] is the service-aware dispatch the manager drives:
//! `HealthProbe` (from `yerd-supervise`, shared with PHP) only sees the listen
//! address, which cannot tell the protocols apart, so [`ServiceProbes`] selects
//! the right per-protocol probe from the type's [`ReadinessKind`].

use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use yerd_supervise::{HealthProbe, Listen};

use crate::service::ReadinessKind;

/// A service-aware readiness probe.
///
/// The manager calls this with the [`ReadinessKind`] of the type it is
/// supervising; the implementation picks the protocol probe to run against
/// `listen`. Kept separate from `yerd_supervise::HealthProbe` (which is
/// address-only and shared with the PHP supervisor) so that trait is untouched.
#[async_trait]
pub trait ReadinessProbe: Send + Sync + 'static {
    /// Probe `kind` at `listen`, returning `Ok(())` once it answers its protocol.
    async fn probe(&self, kind: ReadinessKind, listen: &Listen) -> Result<(), io::Error>;
}

/// The production [`ReadinessProbe`]: dispatches by [`ReadinessKind`] to the
/// matching protocol probe. A unit struct - the per-protocol probes are
/// themselves unit structs.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceProbes;

impl ServiceProbes {
    /// Construct the dispatcher.
    #[must_use]
    pub const fn new() -> Self {
        ServiceProbes
    }
}

#[async_trait]
impl ReadinessProbe for ServiceProbes {
    async fn probe(&self, kind: ReadinessKind, listen: &Listen) -> Result<(), io::Error> {
        match kind {
            ReadinessKind::RedisPing => RedisProbe.probe(listen).await,
            ReadinessKind::MySqlHandshake => MySqlProbe.probe(listen).await,
            ReadinessKind::PostgresStartup => PostgresProbe.probe(listen).await,
            ReadinessKind::MeilisearchHealth => MeilisearchProbe.probe(listen).await,
            ReadinessKind::TcpConnect => TcpConnectProbe.probe(listen).await,
        }
    }
}

/// Meilisearch readiness probe: require a successful health endpoint and the
/// documented available status, rather than treating an open HTTP port as ready.
pub struct MeilisearchProbe;

#[async_trait]
impl HealthProbe for MeilisearchProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let addr = tcp_addr(listen)?;
        let mut stream = TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed HTTP response"))?;
        let headers = std::str::from_utf8(&response[..split])
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 HTTP headers"))?;
        let status = headers.lines().next().unwrap_or_default();
        if !status.starts_with("HTTP/1.1 200 ") && !status.starts_with("HTTP/1.0 200 ") {
            return Err(io::Error::other(
                "Meilisearch health returned non-200 status",
            ));
        }
        let body = response.get(split + 4..).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if value.get("status").and_then(serde_json::Value::as_str) == Some("available") {
            Ok(())
        } else {
            Err(io::Error::other("Meilisearch is not available"))
        }
    }
}

/// Extract the TCP loopback address from a [`Listen`], erroring for a Unix
/// socket (services always listen on fixed loopback TCP).
fn tcp_addr(listen: &Listen) -> Result<SocketAddr, io::Error> {
    match listen {
        Listen::TcpLoopback(a) => Ok(*a),
        Listen::UnixSocket(_) => Err(io::Error::other(
            "service probe requires a TCP listen address",
        )),
    }
}

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

/// App-server readiness probe (Reverb): a bare TCP connect succeeding means the
/// listener is open and accepting connections. Reverb opens its socket only once
/// its event loop is ready to serve, so a successful connect is a sound readiness
/// signal without speaking the WebSocket protocol.
pub struct TcpConnectProbe;

#[async_trait]
impl HealthProbe for TcpConnectProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let addr = tcp_addr(listen)?;
        let _stream = TcpStream::connect(addr).await?;
        Ok(())
    }
}

/// `MySQL` / `MariaDB` readiness probe: connect and **read** the server's
/// initial handshake packet. A bare TCP connect is not enough - during datadir
/// init / crash recovery the listener may already accept connections without
/// yet sending the greeting, so require an actual handshake byte.
///
/// `MySQL` packet framing: a 3-byte little-endian length + 1-byte sequence id,
/// then the payload. For the server greeting the payload's first byte is the
/// protocol version (`0x0a` = v10). A connection-refusal error packet
/// (`0xff`) also proves the server is up and speaking, so we accept that too.
pub struct MySqlProbe;

#[async_trait]
impl HealthProbe for MySqlProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let addr = tcp_addr(listen)?;
        let mut stream = TcpStream::connect(addr).await?;
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await?;
        match buf[4] {
            0x0a | 0xff => Ok(()),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected MySQL handshake first byte {other:#04x}"),
            )),
        }
    }
}

/// `PostgreSQL` readiness probe: send a minimal `StartupMessage` and require any
/// well-formed reply. With `--auth=trust` the server answers `R`
/// (Authentication → `AuthenticationOk`); a refusal answers `E` (`ErrorResponse`),
/// either of which proves the postmaster is up and processing the protocol. The
/// reduced (zonky-style) build may ship no `pg_isready`, so a protocol probe is
/// required rather than a CLI shell-out.
pub struct PostgresProbe;

#[async_trait]
impl HealthProbe for PostgresProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        let addr = tcp_addr(listen)?;
        let mut stream = TcpStream::connect(addr).await?;

        let params: &[u8] = b"user\0postgres\0database\0postgres\0\0";
        let len = u32::try_from(8 + params.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "startup message too large"))?;
        let mut msg = Vec::with_capacity(len as usize);
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(&196_608u32.to_be_bytes());
        msg.extend_from_slice(params);
        stream.write_all(&msg).await?;

        let mut tag = [0u8; 1];
        stream.read_exact(&mut tag).await?;
        match tag[0] {
            b'R' | b'E' => Ok(()),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected Postgres reply tag {:?}", other as char),
            )),
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

    /// Spin a one-shot fake server that, on accept, writes `reply`.
    async fn fake_server(reply: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut scratch = [0u8; 64];
                let _ = sock.try_read(&mut scratch);
                let _ = sock.write_all(reply).await;
                let _ = sock.read(&mut scratch).await;
            }
        });
        addr
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mysql_probe_accepts_handshake() {
        let addr = fake_server(&[0x20, 0x00, 0x00, 0x00, 0x0a, 0xde, 0xad]).await;
        assert!(MySqlProbe.probe(&Listen::TcpLoopback(addr)).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mysql_probe_accepts_err_packet() {
        let addr = fake_server(&[0x10, 0x00, 0x00, 0x00, 0xff, 0x15, 0x04]).await;
        assert!(MySqlProbe.probe(&Listen::TcpLoopback(addr)).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mysql_probe_rejects_garbage() {
        let addr = fake_server(&[0x00, 0x00, 0x00, 0x00, 0x00]).await;
        assert!(MySqlProbe.probe(&Listen::TcpLoopback(addr)).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn postgres_probe_accepts_authentication() {
        let addr = fake_server(b"R\x00\x00\x00\x08\x00\x00\x00\x00").await;
        assert!(PostgresProbe
            .probe(&Listen::TcpLoopback(addr))
            .await
            .is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn postgres_probe_accepts_error_response() {
        let addr = fake_server(b"E\x00\x00\x00\x05X").await;
        assert!(PostgresProbe
            .probe(&Listen::TcpLoopback(addr))
            .await
            .is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn postgres_probe_rejects_garbage() {
        let addr = fake_server(b"Z garbage").await;
        assert!(PostgresProbe
            .probe(&Listen::TcpLoopback(addr))
            .await
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn service_probes_dispatches_by_kind() {
        let redis = fake_server(b"+PONG\r\n").await;
        let probes = ServiceProbes::new();
        assert!(probes
            .probe(ReadinessKind::RedisPing, &Listen::TcpLoopback(redis))
            .await
            .is_ok());

        let mysql = fake_server(&[0x20, 0x00, 0x00, 0x00, 0x0a]).await;
        assert!(probes
            .probe(ReadinessKind::MySqlHandshake, &Listen::TcpLoopback(mysql))
            .await
            .is_ok());

        let pg = fake_server(b"R\x00\x00\x00\x08\x00\x00\x00\x00").await;
        assert!(probes
            .probe(ReadinessKind::PostgresStartup, &Listen::TcpLoopback(pg))
            .await
            .is_ok());
    }

    async fn fake_http(status: &'static str, body: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut request = [0u8; 256];
                let _ = sock.read(&mut request).await;
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        addr
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn meilisearch_probe_requires_healthy_200_response() {
        let healthy = fake_http("200 OK", r#"{"status":"available"}"#).await;
        assert!(MeilisearchProbe
            .probe(&Listen::TcpLoopback(healthy))
            .await
            .is_ok());
        for (status, body) in [
            ("503 Service Unavailable", r#"{"status":"available"}"#),
            ("200 OK", r#"{"status":"unavailable"}"#),
            ("200 OK", "not json"),
        ] {
            let addr = fake_http(status, body).await;
            assert!(MeilisearchProbe
                .probe(&Listen::TcpLoopback(addr))
                .await
                .is_err());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_connect_probe_succeeds_when_listener_open() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(TcpConnectProbe
            .probe(&Listen::TcpLoopback(addr))
            .await
            .is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_connect_probe_fails_when_nothing_listening() {
        let dead: SocketAddr = (Ipv4Addr::LOCALHOST, 1).into();
        assert!(TcpConnectProbe
            .probe(&Listen::TcpLoopback(dead))
            .await
            .is_err());
    }
}
