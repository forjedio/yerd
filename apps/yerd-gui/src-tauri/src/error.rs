//! The error type returned by every Tauri command.
//!
//! It serialises as `{ "code": "...", "message": "..." }` so the frontend's
//! `IpcError` (client.ts) can categorise failures (e.g. `unreachable` flips the
//! "Daemon not running" card). Keep it logic-free: it only carries a category
//! and a human string.

use serde::{Serialize, Serializer};

/// A failure surfaced to the webview from a command.
#[derive(Debug, Clone)]
pub struct GuiError {
    /// Machine-readable category: a daemon `ErrorCode` (snake_case),
    /// `"unreachable"`, or `"internal"`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

impl GuiError {
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self {
            code: "unreachable".to_owned(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "internal".to_owned(),
            message: message.into(),
        }
    }

    pub fn daemon(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for GuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for GuiError {}

// Manual Serialize so the wire shape is exactly `{ code, message }` regardless
// of how `thiserror`/derive might otherwise lay it out.
impl Serialize for GuiError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("GuiError", 2)?;
        s.serialize_field("code", &self.code)?;
        s.serialize_field("message", &self.message)?;
        s.end()
    }
}
