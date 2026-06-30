//! Cloudflare Tunnel support for Yerd.
//!
//! Lets a local `.test` site be published through Cloudflare's edge via
//! `cloudflared` — outbound-only, unprivileged. Modeled on `yerd-php`: the
//! [`origin`], [`args`], [`parse`], and [`config`] submodules are **pure** (sync,
//! no I/O, table-tested), while [`manager`] is the async I/O edge that supervises
//! the `cloudflared` child via the shared `yerd-supervise` state machine.
//!
//! Two tunnel tiers share this machinery (see [`TunnelKind`]): ephemeral Quick
//! Tunnels (random `*.trycloudflare.com` URL, no account) and Named Tunnels
//! (stable hostname on the user's Cloudflare domain).

#![forbid(unsafe_code)]

pub mod args;
pub mod config;
pub mod error;
pub mod manager;
pub mod origin;
pub mod parse;

pub use error::TunnelError;
pub use manager::{TunnelManager, TunnelSnapshot, TunnelState};
pub use origin::{OriginTarget, Scheme};

/// Which tunnel tier a supervised `cloudflared` instance is serving.
///
/// The two tiers drive the same supervisor with different command args and a
/// different readiness signal (see [`parse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelKind {
    /// Ephemeral `*.trycloudflare.com` tunnel; ready when its URL is printed.
    Quick,
    /// Named tunnel on the user's domain; ready when the edge connection
    /// registers.
    Named,
}
