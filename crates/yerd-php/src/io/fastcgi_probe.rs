//! Real `HealthProbe` impl: sends one liveness request and reads back any
//! record-shaped reply.
//!
//! The probe distinguishes "TCP accept queue with nothing behind it" (a Windows
//! edge case) from "the server responded" by validating the FCGI header version
//! on the reply, so a bare connect is never enough.
//!
//! **The request differs by host, because the server does.** PHP-FPM answers the
//! `FCGI_GET_VALUES` management record, so on Unix that is the cheapest possible
//! ping. `php-cgi.exe` does not implement it: it accepts the connection, reads
//! the record, and replies with nothing at all. Probing it that way blocks until
//! the caller's timeout on every attempt, so the supervisor never sees a healthy
//! start, kills the process once its health-check window elapses, and reports a
//! crash loop for a server that was serving correctly the whole time. Windows
//! therefore sends the smallest *real* responder request instead, which php-cgi
//! answers immediately.

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::listen::Listen;
use crate::traits::HealthProbe;

const FCGI_VERSION_1: u8 = 1;
const FCGI_BEGIN_REQUEST: u8 = 1;
const FCGI_PARAMS: u8 = 4;
const FCGI_STDIN: u8 = 5;
const FCGI_GET_VALUES: u8 = 9;
/// `FCGI_RESPONDER`, the only role a PHP `FastCGI` server implements.
const FCGI_RESPONDER: u8 = 1;
/// Request id for the probe's request. Must be non-zero: id 0 is reserved for
/// management records such as [`FCGI_GET_VALUES`].
const PROBE_REQUEST_ID: u16 = 1;

/// Production [`HealthProbe`] impl.
pub struct FastCgiProbe;

#[async_trait]
impl HealthProbe for FastCgiProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        match listen {
            #[cfg(unix)]
            Listen::UnixSocket(path) => {
                let mut s = tokio::net::UnixStream::connect(path).await?;
                send_probe(&mut s).await?;
                read_one_record_header(&mut s).await
            }
            #[cfg(not(unix))]
            Listen::UnixSocket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "UnixSocket listen on non-Unix",
            )),
            Listen::TcpLoopback(addr) => {
                let mut s = tokio::net::TcpStream::connect(addr).await?;
                send_probe(&mut s).await?;
                read_one_record_header(&mut s).await
            }
        }
    }
}

/// The liveness request this host's `FastCGI` server actually answers: a real
/// responder request for `php-cgi` (Windows), the cheaper `FCGI_GET_VALUES`
/// management record for PHP-FPM (see the module docs).
///
/// Both arms compile everywhere so both stay unit-tested on every OS.
async fn send_probe<S>(s: &mut S) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    if cfg!(windows) {
        send_begin_request(s).await
    } else {
        send_get_values(s).await
    }
}

/// An 8-byte `FastCGI` record header. Content length and request id are
/// big-endian per the spec.
#[must_use]
fn record_header(record_type: u8, request_id: u16, content_len: u16) -> [u8; 8] {
    let [id_hi, id_lo] = request_id.to_be_bytes();
    let [len_hi, len_lo] = content_len.to_be_bytes();
    // version, type, requestIdB1, requestIdB0, contentLengthB1,
    // contentLengthB0, paddingLength, reserved
    [
        FCGI_VERSION_1,
        record_type,
        id_hi,
        id_lo,
        len_hi,
        len_lo,
        0,
        0,
    ]
}

/// Write an 8-byte `FCGI_GET_VALUES` request with an empty body.
async fn send_get_values<S>(s: &mut S) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    s.write_all(&record_header(FCGI_GET_VALUES, 0, 0)).await?;
    s.flush().await?;
    Ok(())
}

/// Write the smallest complete responder request: `BEGIN_REQUEST`, then the
/// empty `PARAMS` and `STDIN` records that close both input streams so the
/// server replies at once instead of waiting for more input. No script is
/// named, so PHP answers "No input file specified." on `FCGI_STDOUT` - a
/// record-shaped reply, which is all the probe needs to prove something real is
/// listening.
async fn send_begin_request<S>(s: &mut S) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&record_header(FCGI_BEGIN_REQUEST, PROBE_REQUEST_ID, 8));
    // BEGIN_REQUEST body: role (big-endian u16), flags, five reserved bytes.
    // Flags stay 0, so the server closes the connection when it is done.
    out.extend_from_slice(&[0, FCGI_RESPONDER, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&record_header(FCGI_PARAMS, PROBE_REQUEST_ID, 0));
    out.extend_from_slice(&record_header(FCGI_STDIN, PROBE_REQUEST_ID, 0));
    s.write_all(&out).await?;
    s.flush().await?;
    Ok(())
}

/// Read exactly 8 bytes and validate the version byte. Anything shorter
/// or with `version != 1` is reported as `io::ErrorKind::Other`.
async fn read_one_record_header<S>(s: &mut S) -> io::Result<()>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = [0u8; 8];
    s.read_exact(&mut buf).await?;
    if buf[0] != FCGI_VERSION_1 {
        return Err(io::Error::other(format!(
            "unexpected FCGI version {}",
            buf[0]
        )));
    }
    Ok(())
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
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn send_get_values_writes_8_byte_header() {
        let (mut a, mut b) = tokio::io::duplex(64);
        send_get_values(&mut a).await.unwrap();
        let mut got = [0u8; 8];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(got, [1, 9, 0, 0, 0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn read_one_record_header_accepts_version_1() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let bytes = [1u8, 10, 0, 0, 0, 0, 0, 0];
        b.write_all(&bytes).await.unwrap();
        b.flush().await.unwrap();
        drop(b);
        read_one_record_header(&mut a).await.unwrap();
    }

    #[tokio::test]
    async fn read_one_record_header_rejects_bad_version() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let bytes = [0u8, 10, 0, 0, 0, 0, 0, 0];
        b.write_all(&bytes).await.unwrap();
        b.flush().await.unwrap();
        drop(b);
        let err = read_one_record_header(&mut a).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[tokio::test]
    async fn read_one_record_header_rejects_short_read() {
        let (mut a, mut b) = tokio::io::duplex(64);
        b.write_all(&[1u8, 10, 0]).await.unwrap();
        drop(b);
        let err = read_one_record_header(&mut a).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    /// End-to-end over a real `TcpListener`, exercising the `TcpLoopback` branch
    /// of `probe` (the one Windows uses, since there is no Unix socket there). A
    /// tiny server reads whichever liveness request this host sends and answers
    /// with a valid version-1 header.
    #[tokio::test]
    async fn probe_over_tcp_loopback_accepts_a_fcgi_v1_reply() {
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut head = [0u8; 8];
            sock.read_exact(&mut head).await.unwrap();
            assert_eq!(head[0], FCGI_VERSION_1);
            if cfg!(windows) {
                // BEGIN_REQUEST, then its 8-byte body plus two empty records.
                assert_eq!(head[1], FCGI_BEGIN_REQUEST);
                let mut rest = [0u8; 24];
                sock.read_exact(&mut rest).await.unwrap();
            } else {
                assert_eq!(head[1], FCGI_GET_VALUES);
            }
            sock.write_all(&[FCGI_VERSION_1, 10, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            sock.flush().await.unwrap();
        });

        FastCgiProbe
            .probe(&Listen::TcpLoopback(addr))
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[test]
    fn record_header_is_big_endian_and_version_1() {
        assert_eq!(
            record_header(FCGI_PARAMS, 1, 0),
            [FCGI_VERSION_1, FCGI_PARAMS, 0, 1, 0, 0, 0, 0]
        );
        assert_eq!(
            record_header(FCGI_STDIN, 0x0102, 0x0304),
            [FCGI_VERSION_1, FCGI_STDIN, 1, 2, 3, 4, 0, 0]
        );
    }

    /// The exact bytes php-cgi needs to answer: a responder `BEGIN_REQUEST` whose
    /// PARAMS and STDIN streams are both closed immediately. Pinned because a
    /// missing stream terminator makes the server wait for more input, which
    /// reproduces the original hang the request form exists to avoid.
    #[tokio::test]
    async fn begin_request_closes_both_input_streams() {
        let mut out: Vec<u8> = Vec::new();
        send_begin_request(&mut out).await.unwrap();

        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(&[FCGI_VERSION_1, FCGI_BEGIN_REQUEST, 0, 1, 0, 8, 0, 0]);
        expected.extend_from_slice(&[0, FCGI_RESPONDER, 0, 0, 0, 0, 0, 0]);
        expected.extend_from_slice(&[FCGI_VERSION_1, FCGI_PARAMS, 0, 1, 0, 0, 0, 0]);
        expected.extend_from_slice(&[FCGI_VERSION_1, FCGI_STDIN, 0, 1, 0, 0, 0, 0]);
        assert_eq!(out, expected);
    }

    /// The management record keeps request id 0, which the spec reserves for it.
    #[tokio::test]
    async fn get_values_uses_the_management_request_id() {
        let mut out: Vec<u8> = Vec::new();
        send_get_values(&mut out).await.unwrap();
        assert_eq!(out, vec![FCGI_VERSION_1, FCGI_GET_VALUES, 0, 0, 0, 0, 0, 0]);
    }
}
