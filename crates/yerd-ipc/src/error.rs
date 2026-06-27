//! Error types for `yerd-ipc`.
//!
//! Two layers:
//! - [`FrameError`] is the pure framing error (`Clone + Eq`).
//! - [`IpcError`] wraps framing errors and serde errors; not `Clone`
//!   because [`serde_json::Error`] is not. Use [`IpcError::kind`] for a
//!   `Clone + Eq` shadow ([`IpcErrorKind`]) suitable for GUI/Tauri.

use thiserror::Error;

/// Framing errors. Produced by [`crate::encode_frame`] and
/// [`crate::FrameDecoder::next_frame`]. Pure (`Clone + Eq`); the
/// transport layer does **not** produce these directly - EOF is
/// surfaced via [`IpcError::UnexpectedEof`] instead.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameError {
    /// Declared/given size exceeds the configured cap.
    ///
    /// Produced by both [`crate::encode_frame`] (`size = payload.len()`,
    /// `max = caller's max`) and [`crate::FrameDecoder::next_frame`]
    /// (`size = wire-declared length`, `max = decoder's configured max`).
    #[error("frame size {size} exceeds max {max}")]
    TooLarge {
        /// The offending size, in bytes.
        size: u64,
        /// The configured cap, in bytes.
        max: u64,
    },

    /// Payload length does not fit in the 4-byte length prefix.
    ///
    /// Only [`crate::encode_frame`] produces this. Unreachable on
    /// 32-bit hosts.
    #[error("payload size {size} overflows the 4-byte length prefix")]
    PayloadOverflowsLengthPrefix {
        /// The offending size, in bytes.
        size: u64,
    },
}

/// Top-level IPC error.
///
/// Not `Clone`/`Eq` because [`serde_json::Error`] is not. Use
/// [`IpcError::kind`] to get an [`IpcErrorKind`] shadow that is.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IpcError {
    /// `serde_json` failed to serialise a value.
    #[error("encode: {0}")]
    Encode(#[source] serde_json::Error),

    /// `serde_json` failed to deserialise bytes.
    #[error("decode: {0}")]
    Decode(#[source] serde_json::Error),

    /// A framing error bubbled up.
    #[error("frame: {0}")]
    Frame(#[from] FrameError),

    /// EOF reached mid-frame with `bytes` already buffered.
    ///
    /// Produced only by the `transport` feature's read helpers; the
    /// pure codec cannot synthesise this.
    #[error("unexpected EOF after {bytes} buffered bytes")]
    UnexpectedEof {
        /// Bytes that were already buffered when EOF arrived.
        bytes: usize,
    },

    /// Underlying I/O failed.
    ///
    /// Produced only by the `transport` feature's read/write helpers.
    /// Carries [`std::io::ErrorKind`] (which is `Copy + Eq`) so the
    /// shadow [`IpcErrorKind`] can be `Clone + Eq` without dragging
    /// in a non-cloneable [`std::io::Error`].
    #[error("io: {kind}")]
    Io {
        /// The OS-level error category.
        kind: std::io::ErrorKind,
    },
}

/// `Clone + Eq` shadow of [`IpcError`] suitable for GUI/Tauri command
/// returns where [`serde_json::Error`] cannot be cloned.
///
/// The [`IpcErrorKind::FrameOther`] catch-all carries the `Display`
/// rendering of any future [`FrameError`] variant added before its
/// paired `IpcErrorKind` variant lands. See the
/// `frame_error_to_kind_is_exhaustive` test in this module for the
/// invariant that prevents that drift.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IpcErrorKind {
    /// Maps to [`IpcError::Encode`]. The inner `serde_json::Error`
    /// detail is intentionally not preserved.
    Encode,
    /// Maps to [`IpcError::Decode`]. The inner `serde_json::Error`
    /// detail is intentionally not preserved.
    Decode,
    /// Maps to [`FrameError::TooLarge`].
    FrameTooLarge {
        /// Mirrors [`FrameError::TooLarge::size`].
        size: u64,
        /// Mirrors [`FrameError::TooLarge::max`].
        max: u64,
    },
    /// Maps to [`FrameError::PayloadOverflowsLengthPrefix`].
    FramePayloadOverflowsLengthPrefix {
        /// Mirrors [`FrameError::PayloadOverflowsLengthPrefix::size`].
        size: u64,
    },
    /// Catch-all for any future [`FrameError`] variant added after this
    /// commit but before [`IpcErrorKind`] is bumped to mirror it.
    /// Carries the `Display` rendering so the information is not lost.
    FrameOther {
        /// The `Display` rendering of the underlying [`FrameError`].
        description: String,
    },
    /// Maps to [`IpcError::UnexpectedEof`].
    UnexpectedEof {
        /// Mirrors [`IpcError::UnexpectedEof::bytes`].
        bytes: usize,
    },
    /// Maps to [`IpcError::Io`].
    Io {
        /// Mirrors [`IpcError::Io::kind`].
        kind: std::io::ErrorKind,
    },
}

impl std::fmt::Display for IpcErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode => write!(f, "encode error"),
            Self::Decode => write!(f, "decode error"),
            Self::FrameTooLarge { size, max } => {
                write!(f, "frame size {size} exceeds max {max}")
            }
            Self::FramePayloadOverflowsLengthPrefix { size } => {
                write!(f, "payload size {size} overflows the 4-byte length prefix")
            }
            Self::FrameOther { description } => write!(f, "frame: {description}"),
            Self::UnexpectedEof { bytes } => {
                write!(f, "unexpected EOF after {bytes} buffered bytes")
            }
            Self::Io { kind } => write!(f, "io: {kind}"),
        }
    }
}

impl IpcError {
    /// Pattern-matchable, `Clone + Eq` shadow of this error.
    ///
    /// Allocates only on the [`IpcErrorKind::FrameOther`] fallback
    /// path, which is reachable solely when a new [`FrameError`]
    /// variant has been added without a paired `IpcErrorKind` variant
    /// in the same commit.
    #[must_use]
    pub fn kind(&self) -> IpcErrorKind {
        match self {
            Self::Encode(_) => IpcErrorKind::Encode,
            Self::Decode(_) => IpcErrorKind::Decode,
            #[allow(unreachable_patterns)]
            Self::Frame(fe) => match fe {
                FrameError::TooLarge { size, max } => IpcErrorKind::FrameTooLarge {
                    size: *size,
                    max: *max,
                },
                FrameError::PayloadOverflowsLengthPrefix { size } => {
                    IpcErrorKind::FramePayloadOverflowsLengthPrefix { size: *size }
                }
                other => IpcErrorKind::FrameOther {
                    description: other.to_string(),
                },
            },
            Self::UnexpectedEof { bytes } => IpcErrorKind::UnexpectedEof { bytes: *bytes },
            Self::Io { kind } => IpcErrorKind::Io { kind: *kind },
        }
    }

    /// Allocates once. Prefer [`IpcError::kind`] + [`std::fmt::Display`]
    /// in hot paths.
    #[must_use]
    pub fn message(&self) -> String {
        self.to_string()
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

    // Forces a build break (or test failure) when a new FrameError
    // variant lands without a matching IpcErrorKind mapping in
    // IpcError::kind(). The match below is exhaustive on FrameError
    // (which is in-crate, so #[non_exhaustive] does not block it).
    #[test]
    fn frame_error_to_kind_is_exhaustive() {
        fn assert_paired(e: FrameError) {
            let kind = IpcError::Frame(e.clone()).kind();
            match e {
                FrameError::TooLarge { .. } => {
                    assert!(matches!(kind, IpcErrorKind::FrameTooLarge { .. }));
                }
                FrameError::PayloadOverflowsLengthPrefix { .. } => {
                    assert!(matches!(
                        kind,
                        IpcErrorKind::FramePayloadOverflowsLengthPrefix { .. }
                    ));
                }
            }
        }
        assert_paired(FrameError::TooLarge { size: 1, max: 0 });
        assert_paired(FrameError::PayloadOverflowsLengthPrefix { size: u64::MAX });
    }

    #[test]
    fn unexpected_eof_maps_to_kind() {
        let e = IpcError::UnexpectedEof { bytes: 17 };
        assert_eq!(e.kind(), IpcErrorKind::UnexpectedEof { bytes: 17 });
    }

    #[test]
    fn io_maps_to_kind() {
        let e = IpcError::Io {
            kind: std::io::ErrorKind::ConnectionReset,
        };
        assert_eq!(
            e.kind(),
            IpcErrorKind::Io {
                kind: std::io::ErrorKind::ConnectionReset
            }
        );
    }

    #[test]
    fn ipc_error_kind_display_matches_thiserror_format() {
        let cases: &[(IpcErrorKind, &str)] = &[
            (IpcErrorKind::Encode, "encode error"),
            (IpcErrorKind::Decode, "decode error"),
            (
                IpcErrorKind::FrameTooLarge { size: 5, max: 4 },
                "frame size 5 exceeds max 4",
            ),
            (
                IpcErrorKind::FramePayloadOverflowsLengthPrefix { size: u64::MAX },
                "payload size 18446744073709551615 overflows the 4-byte length prefix",
            ),
            (
                IpcErrorKind::FrameOther {
                    description: "x".into(),
                },
                "frame: x",
            ),
            (
                IpcErrorKind::UnexpectedEof { bytes: 17 },
                "unexpected EOF after 17 buffered bytes",
            ),
            (
                IpcErrorKind::Io {
                    kind: std::io::ErrorKind::BrokenPipe,
                },
                "io: broken pipe",
            ),
        ];
        for (kind, want) in cases {
            assert_eq!(&kind.to_string(), want, "kind = {kind:?}");
        }
    }

    #[test]
    fn frame_error_display_unchanged_by_thiserror() {
        let e = FrameError::TooLarge { size: 17, max: 16 };
        assert_eq!(e.to_string(), "frame size 17 exceeds max 16");
        let e = FrameError::PayloadOverflowsLengthPrefix { size: 1 << 33 };
        assert_eq!(
            e.to_string(),
            "payload size 8589934592 overflows the 4-byte length prefix"
        );
    }

    #[test]
    fn ipc_error_message_equals_display() {
        let e = IpcError::Frame(FrameError::TooLarge { size: 5, max: 4 });
        assert_eq!(e.message(), e.to_string());
    }
}
