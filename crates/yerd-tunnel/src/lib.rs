//! Cloudflare Tunnel support for Yerd.
//!
//! Lets a local `.test` site be published through Cloudflare's edge via
//! `cloudflared`: outbound-only, unprivileged. Modeled on `yerd-php`: the
//! [`origin`], [`args`], [`parse`], [`config`], and [`preflight`] submodules are **pure** (sync,
//! no I/O, table-tested), while [`manager`] is the async I/O edge that supervises
//! the `cloudflared` child via the shared `yerd-supervise` state machine and
//! [`probe`] checks its edge health through the `/ready` metrics endpoint.
//! [`providers`] holds the pure argv and output helpers for the ngrok and zrok
//! one-shot commands.
//!
//! Every tunnel points `cloudflared` at the daemon's loopback tunnel listener
//! ([`origin::origin_url`]), which routes by the public `Host` itself, so no
//! `Host` rewrite or origin TLS options are generated.
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
pub mod preflight;
pub mod probe;
pub mod providers;

pub use error::TunnelError;
pub use manager::{
    LaunchSpec, MetricsAddrSource, Step, TunnelManager, TunnelSnapshot, TunnelState,
};
pub use origin::origin_url;
pub use preflight::{preflight, ShareFacts};
pub use probe::CloudflaredReadyProbe;

/// Which tunnel tier a supervised `cloudflared` instance is serving.
///
/// The two tiers drive the same supervisor with different command args. Both
/// need a `/ready` 200; a Quick tunnel also needs its URL in the log (see
/// [`parse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelKind {
    /// Ephemeral `*.trycloudflare.com` tunnel; ready once its URL is printed
    /// and the edge is connected.
    Quick,
    /// Named tunnel on the user's domain; ready once the edge is connected.
    Named,
}
