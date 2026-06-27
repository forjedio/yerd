//! Length-prefixed frame codec.
//!
//! Wire format: a 4-byte big-endian `u32` length prefix followed by
//! that many bytes of payload. The codec itself is byte-agnostic - it
//! takes/returns `&[u8]` / `Vec<u8>` and never inspects payload
//! contents.
//!
//! The codec is **pure**: no I/O, no async, no allocations beyond the
//! decoder's internal buffer and `encode_frame`'s returned `Vec`. The
//! transport layer (gated behind the `transport` feature) wraps these
//! with `tokio` `AsyncRead`/`AsyncWrite` helpers.

use crate::error::FrameError;

/// 16 MiB - the default maximum frame size on both sides.
pub const DEFAULT_MAX_FRAME: usize = 16 * 1024 * 1024;

/// Size of the length prefix, in bytes.
const HEADER_LEN: usize = 4;

/// Validate `len` against `max` (inclusive) and narrow it to `u32`.
///
/// `max` is inclusive: `len == max` is allowed. Used by
/// [`encode_frame`] and tested directly in this module's `tests`.
fn check_payload_length(len: usize, max: usize) -> Result<u32, FrameError> {
    let len_u64 = len as u64;
    let max_u64 = max as u64;
    if len > max {
        return Err(FrameError::TooLarge {
            size: len_u64,
            max: max_u64,
        });
    }
    u32::try_from(len).map_err(|_| FrameError::PayloadOverflowsLengthPrefix { size: len_u64 })
}

/// Encode a single frame, capped at `max`.
///
/// The receiver enforces its own `max` via
/// [`FrameDecoder::with_max`]; encoding with a `max` lets the sender
/// catch oversized payloads before they hit the wire. Both sides
/// should agree on a cap. For symmetry, both default to
/// [`DEFAULT_MAX_FRAME`].
///
/// Fails with [`FrameError::TooLarge`] if `payload.len() > max`, or
/// with [`FrameError::PayloadOverflowsLengthPrefix`] if
/// `payload.len()` does not fit in the 4-byte length prefix (only
/// reachable on 64-bit hosts).
pub fn encode_frame(payload: &[u8], max: usize) -> Result<Vec<u8>, FrameError> {
    let len_u32 = check_payload_length(payload.len(), max)?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&len_u32.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Length-prefixed frame decoder.
///
/// Stateful: feed bytes via [`extend_from_slice`](Self::extend_from_slice)
/// and pull complete frames via [`next_frame`](Self::next_frame).
/// Handles partial reads, multiple frames per buffer, and oversized
/// declared lengths.
///
/// **Poisoning.** When `next_frame` rejects an oversized declared
/// length, the decoder is *poisoned*: subsequent `next_frame` calls
/// return the same error and `extend_from_slice` becomes a no-op. The
/// internal buffer is cleared on poison to release memory. Because
/// `buffered()` then returns 0, the transport helper may emit
/// `IpcError::UnexpectedEof { bytes: 0 }` if EOF arrives on a poisoned
/// decoder.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    max: usize,
    poisoned: Option<FrameError>,
}

impl FrameDecoder {
    /// Equivalent to `Self::with_max(DEFAULT_MAX_FRAME)`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max(DEFAULT_MAX_FRAME)
    }

    /// New decoder with the given per-frame cap.
    #[must_use]
    pub fn with_max(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            max,
            poisoned: None,
        }
    }

    /// New decoder with the given per-frame cap and pre-allocated
    /// read-buffer capacity.
    #[must_use]
    pub fn with_max_and_capacity(max: usize, capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            max,
            poisoned: None,
        }
    }

    /// Bytes currently buffered (header + body of an in-flight frame,
    /// plus surplus from pipelined frames). Returns 0 after poisoning.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Append socket bytes. **No-op once the decoder is poisoned.**
    pub fn extend_from_slice(&mut self, chunk: &[u8]) {
        if self.poisoned.is_some() {
            return;
        }
        self.buf.extend_from_slice(chunk);
    }

    /// Pull a frame from the buffer.
    ///
    /// - `Ok(Some(payload))` - one full frame ready; surplus bytes
    ///   stay buffered for the next call.
    /// - `Ok(None)` - header or body still incomplete; feed more bytes.
    /// - `Err(FrameError::TooLarge)` - declared length exceeds `max`.
    ///   The decoder is now poisoned; see the type-level docs.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if let Some(err) = &self.poisoned {
            return Err(err.clone());
        }
        let Some(header) = self.buf.first_chunk::<HEADER_LEN>() else {
            return Ok(None);
        };
        let declared = u32::from_be_bytes(*header) as usize;
        if declared > self.max {
            let err = FrameError::TooLarge {
                size: declared as u64,
                max: self.max as u64,
            };
            self.poisoned = Some(err.clone());
            self.buf.clear();
            self.buf.shrink_to_fit();
            return Err(err);
        }
        let total = HEADER_LEN.saturating_add(declared);
        if self.buf.len() < total {
            return Ok(None);
        }
        let mut consumed: Vec<u8> = self.buf.drain(..total).collect();
        let payload = consumed.split_off(HEADER_LEN);
        Ok(Some(payload))
    }
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
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

    #[test]
    fn check_payload_length_zero_and_zero() {
        assert_eq!(check_payload_length(0, 0).unwrap(), 0);
    }

    #[test]
    fn check_payload_length_zero_under_cap() {
        assert_eq!(check_payload_length(0, 10).unwrap(), 0);
    }

    #[test]
    fn check_payload_length_at_cap_succeeds() {
        assert_eq!(check_payload_length(10, 10).unwrap(), 10);
    }

    #[test]
    fn check_payload_length_one_over_cap_fails() {
        let err = check_payload_length(11, 10).unwrap_err();
        assert_eq!(err, FrameError::TooLarge { size: 11, max: 10 });
    }

    #[test]
    fn check_payload_length_one_byte_against_zero_cap_fails() {
        let err = check_payload_length(1, 0).unwrap_err();
        assert_eq!(err, FrameError::TooLarge { size: 1, max: 0 });
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn check_payload_length_u32_overflow_on_64bit() {
        let len = usize::try_from(u64::from(u32::MAX) + 1).unwrap();
        let err = check_payload_length(len, usize::MAX).unwrap_err();
        assert_eq!(
            err,
            FrameError::PayloadOverflowsLengthPrefix {
                size: u64::from(u32::MAX) + 1
            }
        );
    }
}
