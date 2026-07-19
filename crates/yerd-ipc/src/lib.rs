//! IPC protocol, framing, and codec used between `yerdd` and its
//! clients (the `yerd` CLI and the Tauri GUI).
//!
//! The default build is pure: no sockets, no async, no I/O. Enable
//! the `transport` feature to pull in `tokio`-based async helpers
//! shared by the daemon and the CLI.
//!
//! ## Wire format
//!
//! Every message is a length-prefixed JSON frame: a 4-byte big-endian
//! `u32` length followed by `length` bytes of UTF-8 JSON. The frame
//! codec is byte-agnostic; the JSON shape is pinned in
//! `tests/wire_stability.rs` and the framing edges are pinned in
//! `tests/frame_codec.rs`.
//!
//! ## Version compatibility
//!
//! [`PROTOCOL_VERSION`] is exposed for future use. Until a
//! `Hello`/`Welcome` handshake is added, a client speaking a newer
//! protocol against an older daemon surfaces as [`IpcError::Decode`]
//! when an unknown `type` tag arrives.

mod create;
mod dump;
mod error;
mod frame;
mod message;
mod request;
mod response;
mod status;
mod update;

#[cfg(feature = "transport")]
mod transport;

/// The current IPC protocol version. Bump on any breaking change;
/// add a handshake before doing so.
pub const PROTOCOL_VERSION: u32 = 1;

pub use create::{
    AuthProvider, CreateSiteSpec, Database, Framework, JobId, JobState, JsRuntime, LaravelOptions,
    StarterKit, Testing, WordPressDatabase, WordPressDbEngine, WordPressOptions,
};
pub use dump::{DumpCategory, DumpCounts, DumpEvent, DumpExtStatus};
pub use error::{FrameError, IpcError, IpcErrorKind};
pub use frame::{encode_frame, FrameDecoder, DEFAULT_MAX_FRAME};
pub use message::{decode_message, encode_message};
pub use request::Request;
pub use response::{
    ErrorCode, PhpExtInfo, PhpUpdate, ProxyEntry, ProxyRuleEntry, Response, SiteEntry,
    WordPressAdminUser,
};
pub use status::{
    AddableServiceType, BrowserTrust, CaStatus, CloudflaredSource, CloudflaredStatus,
    DatabaseSummary, Diagnosis, DiagnosisCode, DomainShadow, FixReport, FixResult, MailAttachment,
    MailDetail, MailHeader, MailStatus, MailSummary, NamedTunnelMeta, PhpPoolStatus, PoolRunState,
    PortStatus, ServiceAvailability, ServiceRunState, ServiceStatus, Severity, SiteCounts,
    SiteHostname, StatusReport, ToolStatus, TunnelInfo, TunnelKind, TunnelRunState, UnboundWeb,
    WordPressVersionInfo,
};
pub use update::{Channel, StagedArtifact, UpdateSource};

/// Re-exports of the shared types that travel on the wire. Consumers
/// that need only the IPC surface should `use yerd_ipc::types::*;`
/// instead of depending on `yerd-core` directly.
pub mod types {
    pub use yerd_core::{PhpVersion, Site, SiteKind};
}

#[cfg(feature = "transport")]
pub use transport::{read_frame, read_message, write_message};

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
    fn protocol_version_pinned() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
