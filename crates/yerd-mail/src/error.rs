//! Error type for the mail-capture subsystem.

use std::io;
use std::path::PathBuf;

/// Failures from binding the SMTP listener or reading/writing the store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MailError {
    /// Could not bind the loopback SMTP port. The daemon treats this as
    /// non-fatal (logs a warning and runs with capture not listening).
    #[error("could not bind mail port {port}: {source}")]
    Bind {
        /// The port that failed to bind.
        port: u16,
        /// The underlying OS error.
        source: io::Error,
    },
    /// A filesystem operation on the store failed.
    #[error("mail store I/O at {path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying OS error.
        source: io::Error,
    },
    /// The on-disk index could not be (de)serialised.
    #[error("mail store index: {0}")]
    Index(#[from] serde_json::Error),
}
