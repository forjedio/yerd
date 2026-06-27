//! Hand-rolled `FastCGI` record framing.
//!
//! Pure encode/decode only - the forwarder owns socket reads/writes.

use thiserror::Error;

/// `FastCGI` protocol version (always 1).
pub const FCGI_VERSION: u8 = 1;
/// `role` value in `BeginRequest` for the Responder role.
pub const FCGI_RESPONDER: u16 = 1;
/// FCGI's `content_length` is a `u16` - 65 535 bytes is the hard cap.
pub const FCGI_MAX_PAYLOAD: usize = 65_535;

/// `protocol_status` value indicating a request completed without error.
pub const FCGI_REQUEST_COMPLETE: u8 = 0;

/// FCGI record type.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    /// `FCGI_BEGIN_REQUEST` (1)
    BeginRequest = 1,
    /// `FCGI_ABORT_REQUEST` (2)
    AbortRequest = 2,
    /// `FCGI_END_REQUEST` (3)
    EndRequest = 3,
    /// `FCGI_PARAMS` (4)
    Params = 4,
    /// `FCGI_STDIN` (5)
    Stdin = 5,
    /// `FCGI_STDOUT` (6)
    Stdout = 6,
    /// `FCGI_STDERR` (7)
    Stderr = 7,
    /// `FCGI_GET_VALUES` (9)
    GetValues = 9,
    /// `FCGI_GET_VALUES_RESULT` (10)
    GetValuesResult = 10,
    /// `FCGI_UNKNOWN_TYPE` (11)
    UnknownType = 11,
}

impl RecordType {
    /// Reverse of the `repr(u8)` cast.
    pub fn from_u8(b: u8) -> Result<Self, FcgiError> {
        Ok(match b {
            1 => Self::BeginRequest,
            2 => Self::AbortRequest,
            3 => Self::EndRequest,
            4 => Self::Params,
            5 => Self::Stdin,
            6 => Self::Stdout,
            7 => Self::Stderr,
            9 => Self::GetValues,
            10 => Self::GetValuesResult,
            11 => Self::UnknownType,
            other => return Err(FcgiError::UnknownRecordType(other)),
        })
    }
}

/// 8-byte FCGI record header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// FCGI protocol version (always 1).
    pub version: u8,
    /// What this record represents.
    pub record_type: RecordType,
    /// Logical request multiplex id. We always use 1.
    pub request_id: u16,
    /// Bytes of content in the payload.
    pub content_length: u16,
    /// Padding bytes appended after the payload.
    pub padding_length: u8,
}

impl Header {
    /// Write the 8 header bytes into `out`.
    pub fn encode(self, out: &mut Vec<u8>) {
        out.push(self.version);
        out.push(self.record_type as u8);
        out.extend_from_slice(&self.request_id.to_be_bytes());
        out.extend_from_slice(&self.content_length.to_be_bytes());
        out.push(self.padding_length);
        out.push(0); // reserved
    }

    /// Decode an 8-byte slice.
    pub fn decode(bytes: &[u8]) -> Result<Self, FcgiError> {
        let arr: &[u8; 8] = bytes.try_into().map_err(|_| FcgiError::Short)?;
        let version = arr[0];
        if version != FCGI_VERSION {
            return Err(FcgiError::BadVersion(version));
        }
        let record_type = RecordType::from_u8(arr[1])?;
        let request_id = u16::from_be_bytes([arr[2], arr[3]]);
        let content_length = u16::from_be_bytes([arr[4], arr[5]]);
        let padding_length = arr[6];
        Ok(Self {
            version,
            record_type,
            request_id,
            content_length,
            padding_length,
        })
    }
}

/// Encode an FCGI name-value pair using FCGI's length-prefix scheme.
///
/// Lengths `<= 127` use a single byte; longer lengths use four bytes with the
/// high bit set on the first byte. Errors if either side exceeds 0x7FFFFFFF.
pub fn encode_name_value(name: &[u8], value: &[u8], out: &mut Vec<u8>) -> Result<(), FcgiError> {
    encode_length(name.len(), out)?;
    encode_length(value.len(), out)?;
    out.extend_from_slice(name);
    out.extend_from_slice(value);
    Ok(())
}

fn encode_length(len: usize, out: &mut Vec<u8>) -> Result<(), FcgiError> {
    if len <= 127 {
        out.push(u8::try_from(len).unwrap_or(0));
        Ok(())
    } else {
        let len32: u32 = u32::try_from(len).map_err(|_| FcgiError::LengthOverflow)?;
        if len32 > 0x7FFF_FFFF {
            return Err(FcgiError::LengthOverflow);
        }
        let bytes = (len32 | 0x8000_0000).to_be_bytes();
        out.extend_from_slice(&bytes);
        Ok(())
    }
}

/// Build the `FCGI_BEGIN_REQUEST` 8-byte body.
///
/// Bytes: roleB1, roleB0, flags, reserved×5.
#[must_use]
pub fn encode_begin_request_body(role: u16, keep_conn: bool) -> [u8; 8] {
    let role_bytes = role.to_be_bytes();
    let flags = u8::from(keep_conn);
    [role_bytes[0], role_bytes[1], flags, 0, 0, 0, 0, 0]
}

/// FCGI `END_REQUEST` body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndRequest {
    /// Status code from the application.
    pub app_status: u32,
    /// FCGI protocol status (`FCGI_REQUEST_COMPLETE` etc).
    pub protocol_status: u8,
}

impl EndRequest {
    /// Decode the 8 body bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, FcgiError> {
        let arr: &[u8; 8] = bytes.try_into().map_err(|_| FcgiError::Short)?;
        let app_status = u32::from_be_bytes([arr[0], arr[1], arr[2], arr[3]]);
        let protocol_status = arr[4];
        Ok(Self {
            app_status,
            protocol_status,
        })
    }
}

/// Errors from this codec module.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FcgiError {
    /// Header or body shorter than expected.
    #[error("FCGI record too short")]
    Short,
    /// Header `version` byte was not 1.
    #[error("unexpected FCGI version {0}")]
    BadVersion(u8),
    /// Header `type` byte was not a known value.
    #[error("unknown FCGI record type {0}")]
    UnknownRecordType(u8),
    /// A name or value length exceeded the encodable range.
    #[error("FCGI name/value length overflow")]
    LengthOverflow,
    /// A record arrived with a `request_id` other than the expected 1.
    #[error("unexpected FCGI request_id {0} (expected 1)")]
    UnexpectedRequestId(u16),
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
    fn header_round_trip() {
        let h = Header {
            version: 1,
            record_type: RecordType::Stdin,
            request_id: 1,
            content_length: 42,
            padding_length: 6,
        };
        let mut out = Vec::new();
        h.encode(&mut out);
        assert_eq!(out.len(), 8);
        let decoded = Header::decode(&out).unwrap();
        assert_eq!(decoded, h);
    }

    #[test]
    fn header_rejects_bad_version() {
        let bytes = [0u8, 5, 0, 1, 0, 0, 0, 0];
        let err = Header::decode(&bytes).unwrap_err();
        assert!(matches!(err, FcgiError::BadVersion(0)));
    }

    #[test]
    fn header_rejects_short() {
        let err = Header::decode(&[1u8, 5, 0]).unwrap_err();
        assert!(matches!(err, FcgiError::Short));
    }

    #[test]
    fn name_value_short_form() {
        let mut out = Vec::new();
        encode_name_value(b"X", b"yz", &mut out).unwrap();
        assert_eq!(out, vec![1, 2, b'X', b'y', b'z']);
    }

    #[test]
    fn name_value_long_form() {
        let big = vec![b'a'; 200];
        let mut out = Vec::new();
        encode_name_value(&big, b"v", &mut out).unwrap();
        assert_eq!(&out[..4], &[0x80, 0x00, 0x00, 0xC8]);
        assert_eq!(out[4], 1);
        assert_eq!(&out[5..205], big.as_slice());
        assert_eq!(out[205], b'v');
    }

    #[test]
    fn begin_request_body_layout() {
        let body = encode_begin_request_body(FCGI_RESPONDER, false);
        assert_eq!(body, [0, 1, 0, 0, 0, 0, 0, 0]);
        let body = encode_begin_request_body(FCGI_RESPONDER, true);
        assert_eq!(body, [0, 1, 1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn end_request_decode() {
        let bytes = [0, 0, 0, 0, FCGI_REQUEST_COMPLETE, 0, 0, 0];
        let er = EndRequest::decode(&bytes).unwrap();
        assert_eq!(er.app_status, 0);
        assert_eq!(er.protocol_status, FCGI_REQUEST_COMPLETE);
    }

    #[test]
    fn end_request_rejects_short() {
        assert!(matches!(
            EndRequest::decode(&[0, 0, 0]).unwrap_err(),
            FcgiError::Short
        ));
    }

    #[test]
    fn fcgi_max_payload_is_u16_max() {
        assert_eq!(FCGI_MAX_PAYLOAD, u16::MAX as usize);
    }
}
