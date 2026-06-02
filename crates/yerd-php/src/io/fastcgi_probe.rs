//! Real `HealthProbe` impl: sends a single `FCGI_GET_VALUES` record and
//! reads back any record-shaped reply.
//!
//! The probe distinguishes "TCP accept queue with no FPM behind it"
//! (Windows edge case) from "FPM responded" by validating the FCGI header
//! version on the reply.

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::listen::Listen;
use crate::traits::HealthProbe;

const FCGI_VERSION_1: u8 = 1;
const FCGI_GET_VALUES: u8 = 9;

/// Production [`HealthProbe`] impl.
pub struct FastCgiProbe;

#[async_trait]
impl HealthProbe for FastCgiProbe {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error> {
        match listen {
            #[cfg(unix)]
            Listen::UnixSocket(path) => {
                let mut s = tokio::net::UnixStream::connect(path).await?;
                send_get_values(&mut s).await?;
                read_one_record_header(&mut s).await
            }
            #[cfg(not(unix))]
            Listen::UnixSocket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "UnixSocket listen on non-Unix",
            )),
            Listen::TcpLoopback(addr) => {
                let mut s = tokio::net::TcpStream::connect(addr).await?;
                send_get_values(&mut s).await?;
                read_one_record_header(&mut s).await
            }
        }
    }
}

/// Write an 8-byte `FCGI_GET_VALUES` request with an empty body.
async fn send_get_values<S>(s: &mut S) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    // version, type, requestIdB1, requestIdB0, contentLengthB1, contentLengthB0,
    // paddingLength, reserved
    let header: [u8; 8] = [FCGI_VERSION_1, FCGI_GET_VALUES, 0, 0, 0, 0, 0, 0];
    s.write_all(&header).await?;
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
        // Server end writes a valid header.
        let bytes = [1u8, 10, 0, 0, 0, 0, 0, 0]; // type doesn't matter
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
}
