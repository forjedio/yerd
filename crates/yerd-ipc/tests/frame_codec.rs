//! Integration tests for the length-prefixed frame codec.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use yerd_ipc::{encode_frame, FrameDecoder, FrameError, DEFAULT_MAX_FRAME};

const SMALL_MAX: usize = 64;

#[test]
fn default_max_frame_constant_value() {
    assert_eq!(DEFAULT_MAX_FRAME, 16 * 1024 * 1024);
}

#[test]
fn encode_then_decode_single_frame_roundtrip() {
    let payload = b"hello yerd".to_vec();
    let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
    let mut dec = FrameDecoder::new();
    dec.extend_from_slice(&frame);
    assert_eq!(dec.next_frame().unwrap(), Some(payload));
    assert_eq!(dec.next_frame().unwrap(), None);
    assert_eq!(dec.buffered(), 0);
}

#[test]
fn empty_payload_roundtrip() {
    let frame = encode_frame(b"", DEFAULT_MAX_FRAME).unwrap();
    assert_eq!(frame, vec![0, 0, 0, 0]);
    let mut dec = FrameDecoder::new();
    dec.extend_from_slice(&frame);
    assert_eq!(dec.next_frame().unwrap(), Some(vec![]));
}

#[test]
fn decoder_yields_none_on_partial_header() {
    let payload = b"abcd".to_vec();
    let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
    let mut dec = FrameDecoder::new();

    dec.extend_from_slice(&frame[..1]);
    assert_eq!(dec.next_frame().unwrap(), None);
    dec.extend_from_slice(&frame[1..2]);
    assert_eq!(dec.next_frame().unwrap(), None);
    dec.extend_from_slice(&frame[2..3]);
    assert_eq!(dec.next_frame().unwrap(), None);
    dec.extend_from_slice(&frame[3..]);
    assert_eq!(dec.next_frame().unwrap(), Some(payload));
}

#[test]
fn decoder_yields_none_on_partial_body() {
    let payload = b"hello".to_vec();
    let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
    let mut dec = FrameDecoder::new();

    // Header + half body.
    let split = 4 + 2;
    dec.extend_from_slice(&frame[..split]);
    assert_eq!(dec.next_frame().unwrap(), None);
    dec.extend_from_slice(&frame[split..]);
    assert_eq!(dec.next_frame().unwrap(), Some(payload));
}

#[test]
fn decoder_yields_multiple_frames_pipelined() {
    let payloads: Vec<Vec<u8>> = vec![b"one".to_vec(), b"two!!".to_vec(), b"three!!!".to_vec()];
    let mut wire = Vec::new();
    for p in &payloads {
        wire.extend_from_slice(&encode_frame(p, DEFAULT_MAX_FRAME).unwrap());
    }
    let mut dec = FrameDecoder::new();
    dec.extend_from_slice(&wire);
    for p in &payloads {
        assert_eq!(dec.next_frame().unwrap().as_ref(), Some(p));
    }
    assert_eq!(dec.next_frame().unwrap(), None);
}

#[test]
fn decoder_handles_extra_trailing_bytes() {
    let payload = b"frame".to_vec();
    let mut wire = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
    // Append a partial next-frame header (2 of 4 bytes).
    wire.push(0);
    wire.push(0);

    let mut dec = FrameDecoder::new();
    dec.extend_from_slice(&wire);
    assert_eq!(dec.next_frame().unwrap(), Some(payload));
    assert_eq!(dec.next_frame().unwrap(), None);
    assert_eq!(dec.buffered(), 2);
}

#[test]
fn decoder_yields_first_then_rejects_oversized_second() {
    let first = b"ok".to_vec();
    let first_frame = encode_frame(&first, SMALL_MAX).unwrap();
    // Hand-build an oversized declared length: SMALL_MAX + 1.
    let mut oversized_header = ((SMALL_MAX + 1) as u32).to_be_bytes().to_vec();
    let mut wire = first_frame;
    wire.append(&mut oversized_header);

    let mut dec = FrameDecoder::with_max(SMALL_MAX);
    dec.extend_from_slice(&wire);
    assert_eq!(dec.next_frame().unwrap(), Some(first));
    let err = dec.next_frame().unwrap_err();
    assert_eq!(
        err,
        FrameError::TooLarge {
            size: (SMALL_MAX + 1) as u64,
            max: SMALL_MAX as u64,
        }
    );
}

#[test]
fn decoder_rejects_oversized_length() {
    let mut header = ((SMALL_MAX + 1) as u32).to_be_bytes().to_vec();
    let mut dec = FrameDecoder::with_max(SMALL_MAX);
    dec.extend_from_slice(&header);
    let err = dec.next_frame().unwrap_err();
    assert_eq!(
        err,
        FrameError::TooLarge {
            size: (SMALL_MAX + 1) as u64,
            max: SMALL_MAX as u64,
        }
    );
    // Touch header so its allocation isn't optimised out — also ensures
    // header is not consumed by the decoder after the error.
    header.clear();
}

#[test]
fn decoder_stays_poisoned_after_oversized() {
    let header = ((SMALL_MAX + 1) as u32).to_be_bytes().to_vec();
    let mut dec = FrameDecoder::with_max(SMALL_MAX);
    dec.extend_from_slice(&header);
    let err_first = dec.next_frame().unwrap_err();
    // Subsequent calls return the *same* error with the *same* size/max.
    let err_second = dec.next_frame().unwrap_err();
    assert_eq!(err_first, err_second);
    // extend_from_slice is now a no-op; buffered() reflects the post-
    // poison cleared state.
    dec.extend_from_slice(b"junk");
    assert_eq!(dec.buffered(), 0);
    // And next_frame keeps returning the same error.
    assert_eq!(dec.next_frame().unwrap_err(), err_first);
}

#[test]
fn encode_succeeds_at_exact_max_boundary() {
    let payload = vec![0xAB; SMALL_MAX];
    let frame = encode_frame(&payload, SMALL_MAX).unwrap();
    let mut dec = FrameDecoder::with_max(SMALL_MAX);
    dec.extend_from_slice(&frame);
    assert_eq!(dec.next_frame().unwrap(), Some(payload));
}

#[test]
fn decoder_rejects_one_byte_over_boundary() {
    // Hand-build a declared length of exactly SMALL_MAX + 1.
    let header = ((SMALL_MAX + 1) as u32).to_be_bytes().to_vec();
    let mut dec = FrameDecoder::with_max(SMALL_MAX);
    dec.extend_from_slice(&header);
    let err = dec.next_frame().unwrap_err();
    assert_eq!(
        err,
        FrameError::TooLarge {
            size: (SMALL_MAX + 1) as u64,
            max: SMALL_MAX as u64,
        }
    );
}

#[test]
fn encode_rejects_payload_larger_than_max() {
    let payload = vec![0; SMALL_MAX + 1];
    let err = encode_frame(&payload, SMALL_MAX).unwrap_err();
    assert_eq!(
        err,
        FrameError::TooLarge {
            size: (SMALL_MAX + 1) as u64,
            max: SMALL_MAX as u64,
        }
    );
}

#[test]
fn encode_rejects_one_byte_against_zero_max() {
    let err = encode_frame(b"x", 0).unwrap_err();
    assert_eq!(err, FrameError::TooLarge { size: 1, max: 0 });
}

#[test]
fn encode_at_zero_max_with_empty_payload_succeeds() {
    let frame = encode_frame(b"", 0).unwrap();
    assert_eq!(frame, vec![0, 0, 0, 0]);
}

#[test]
fn slow_loris_byte_at_a_time_full_message_roundtrip() {
    let payload: Vec<u8> = (0..=200_u8).collect();
    let frame = encode_frame(&payload, DEFAULT_MAX_FRAME).unwrap();
    let mut dec = FrameDecoder::new();
    let mut result: Option<Vec<u8>> = None;
    for byte in &frame {
        dec.extend_from_slice(std::slice::from_ref(byte));
        if let Some(p) = dec.next_frame().unwrap() {
            result = Some(p);
        }
    }
    assert_eq!(result, Some(payload));
    assert_eq!(dec.next_frame().unwrap(), None);
    assert_eq!(dec.buffered(), 0);
}
