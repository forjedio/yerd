//! Built-in mail-capture SMTP server and on-disk store for Yerd.
//!
//! Herd-style: the daemon runs a tiny SMTP sink on a loopback port; everything
//! it receives is stored as a raw `.eml` file and surfaced (decoded) to the GUI
//! for inspection. There is no relaying - captured mail never leaves the box.
//!
//! ## Purity boundary
//!
//! Like the rest of the workspace, pure logic lives in [`pure`] (the SMTP
//! command state machine, MIME decoding, retention policy - all sync and
//! testable with no I/O) and the side-effecting edges live in [`io`] (the tokio
//! TCP server and the disk store).

#![forbid(unsafe_code)]

pub mod error;
pub mod io;
pub mod pure;

pub use error::MailError;
pub use io::server::{bind, serve};
pub use io::store::Store;
pub use pure::smtp::RawMessage;
