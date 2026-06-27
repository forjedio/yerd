//! Async transport helpers shared by `yerdd` and the `yerd` CLI.
//!
//! Gated behind the `transport` feature so the default build stays
//! runtime-free. The helpers are generic over [`tokio::io::AsyncRead`]
//! / [`tokio::io::AsyncWrite`] - socket and named-pipe binding stays in
//! the binaries.

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{decode_message, encode_frame, encode_message, error::IpcError, frame::FrameDecoder};

/// Per-`read` scratch buffer size. Tuneable.
const READ_CHUNK: usize = 4 * 1024;

/// Serialise `value` to JSON, length-prefix it (capped at `max`), and
/// write it to `writer`.
pub async fn write_message<W, T>(writer: &mut W, value: &T, max: usize) -> Result<(), IpcError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = encode_message(value)?;
    let frame = encode_frame(&payload, max)?;
    writer.write_all(&frame).await.map_err(io_to_ipc)?;
    Ok(())
}

/// Pull one full frame from `reader`, using `decoder` to buffer
/// partial reads and surplus bytes from pipelined frames.
///
/// Returns the raw payload so callers can inspect the `type` tag
/// before fully decoding.
///
/// - `Ok(Some(payload))` - one full frame ready.
/// - `Ok(None)` - clean EOF with an empty decoder buffer.
/// - `Err(IpcError::UnexpectedEof { bytes })` - EOF mid-frame.
/// - `Err(IpcError::Frame(_))` - declared length exceeded the
///   decoder's cap (decoder is now poisoned).
pub async fn read_frame<R>(
    reader: &mut R,
    decoder: &mut FrameDecoder,
) -> Result<Option<Vec<u8>>, IpcError>
where
    R: AsyncRead + Unpin,
{
    loop {
        if let Some(payload) = decoder.next_frame()? {
            return Ok(Some(payload));
        }
        let mut chunk = [0_u8; READ_CHUNK];
        let n = reader.read(&mut chunk).await.map_err(io_to_ipc)?;
        if n == 0 {
            return if decoder.buffered() == 0 {
                Ok(None)
            } else {
                Err(IpcError::UnexpectedEof {
                    bytes: decoder.buffered(),
                })
            };
        }
        if let Some(slice) = chunk.get(..n) {
            decoder.extend_from_slice(slice);
        }
    }
}

/// Convenience: [`read_frame`] then [`decode_message`].
pub async fn read_message<R, T>(
    reader: &mut R,
    decoder: &mut FrameDecoder,
) -> Result<Option<T>, IpcError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    match read_frame(reader, decoder).await? {
        Some(bytes) => Ok(Some(decode_message::<T>(&bytes)?)),
        None => Ok(None),
    }
}

// Preserve only the OS error category. `std::io::ErrorKind` is
// `Copy + Eq` so it composes cleanly with `IpcErrorKind`. Clean EOF
// (read returning 0 bytes) is handled separately in `read_frame` and
// surfaces as `IpcError::UnexpectedEof`.
fn io_to_ipc(e: std::io::Error) -> IpcError {
    IpcError::Io { kind: e.kind() }
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
    use crate::{Request, DEFAULT_MAX_FRAME};
    use std::path::PathBuf;
    use tokio::io::{duplex, AsyncWriteExt};

    #[tokio::test]
    async fn write_then_read_one_message_over_duplex() {
        let (mut a, mut b) = duplex(1024);
        let want = Request::Park {
            path: PathBuf::from("/srv/foo"),
        };
        write_message(&mut a, &want, DEFAULT_MAX_FRAME)
            .await
            .unwrap();
        a.shutdown().await.unwrap();
        drop(a);

        let mut dec = FrameDecoder::new();
        let got: Option<Request> = read_message(&mut b, &mut dec).await.unwrap();
        assert_eq!(got.as_ref(), Some(&want));

        let eof: Option<Request> = read_message(&mut b, &mut dec).await.unwrap();
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn read_blocks_for_partial_frame_then_completes() {
        let (mut a, mut b) = duplex(64);
        let payload = encode_message(&Request::Ping).unwrap();
        let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();

        let writer = tokio::spawn(async move {
            let mid = frame.len() / 2;
            a.write_all(&frame[..mid]).await.unwrap();
            tokio::task::yield_now().await;
            a.write_all(&frame[mid..]).await.unwrap();
            a.shutdown().await.unwrap();
            drop(a);
        });

        let mut dec = FrameDecoder::new();
        let got: Option<Request> = read_message(&mut b, &mut dec).await.unwrap();
        assert_eq!(got, Some(Request::Ping));
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn read_returns_none_at_clean_eof() {
        let (a, mut b) = duplex(8);
        drop(a);
        let mut dec = FrameDecoder::new();
        let got: Option<Request> = read_message(&mut b, &mut dec).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn read_errors_with_unexpected_eof_mid_frame() {
        let (mut a, mut b) = duplex(64);
        let payload = encode_message(&Request::Ping).unwrap();
        let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
        a.write_all(&frame[..4]).await.unwrap();
        a.shutdown().await.unwrap();
        drop(a);

        let mut dec = FrameDecoder::new();
        let err = read_message::<_, Request>(&mut b, &mut dec)
            .await
            .unwrap_err();
        match err {
            IpcError::UnexpectedEof { bytes } => assert!(bytes > 0),
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_to_closed_reader_surfaces_as_io_error() {
        let (mut a, b) = duplex(8);
        drop(b);
        let payload = vec![0_u8; 1024];
        let req = Request::Park {
            path: PathBuf::from(String::from_utf8(payload).unwrap_or_default()),
        };
        let err = write_message(&mut a, &req, DEFAULT_MAX_FRAME)
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::Io { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn write_message_survives_partial_writes_on_small_duplex() {
        let (mut a, mut b) = duplex(8);
        let want = Request::Link {
            name: "alpha".into(),
            path: PathBuf::from("/srv/alpha-with-a-longer-than-buffer-path"),
        };
        let writer = tokio::spawn(async move {
            write_message(&mut a, &want, DEFAULT_MAX_FRAME)
                .await
                .unwrap();
            a.shutdown().await.unwrap();
            drop(a);
            want
        });

        let mut dec = FrameDecoder::new();
        let got: Option<Request> = read_message(&mut b, &mut dec).await.unwrap();
        let want = writer.await.unwrap();
        assert_eq!(got.as_ref(), Some(&want));
    }
}
