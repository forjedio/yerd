//! Tunnel providers for Yerd.
//!
//! Lets a local `.test` site be published through a tunnel provider's edge:
//! Cloudflare (`cloudflared`), ngrok or zrok, outbound-only and unprivileged.
//! Modeled on `yerd-php`: the [`origin`], [`args`], [`parse`], [`config`],
//! [`providers`] and [`preflight`] submodules are **pure** (sync, no I/O,
//! table-tested), while [`manager`] is the async I/O edge that supervises each
//! tunnel child via the shared `yerd-supervise` state machine and [`probe`]
//! checks `cloudflared`'s edge health through the `/ready` metrics endpoint.
//! [`providers`] holds the argv, config and output helpers for ngrok and zrok.
//!
//! Every tunnel points its agent at the daemon's loopback tunnel listener
//! ([`origin::origin_url`]), which routes by the public `Host` itself, so no
//! `Host` rewrite or origin TLS options are generated.
//!
//! What a supervised instance serves is its [`TunnelKind`]: an ephemeral
//! Cloudflare Quick Tunnel (random `*.trycloudflare.com` URL, no account), the
//! Cloudflare Named Tunnel (stable hostnames on the user's domain), an ngrok
//! share, or a zrok public share.

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

/// What a supervised tunnel instance serves.
///
/// Every kind drives the same supervisor with different command args. The two
/// Cloudflare kinds need a `/ready` 200 and a Quick tunnel also needs its URL
/// in the log (see [`parse`]); ngrok and zrok expose no health endpoint, so
/// they are ready once their public URL is in the log (see [`providers`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelKind {
    /// Ephemeral `*.trycloudflare.com` tunnel; ready once its URL is printed
    /// and the edge is connected.
    Quick,
    /// Named tunnel on the user's domain; ready once the edge is connected.
    Named,
    /// An ngrok share; ready once its public URL is logged.
    Ngrok {
        /// Whether it serves the configured static domain rather than the URL
        /// ngrok assigns to the account.
        static_domain: bool,
    },
    /// A zrok public share; ready once its public URL is logged.
    Zrok,
}

impl TunnelKind {
    /// Whether readiness and edge health come from `cloudflared`'s `/ready`
    /// probe (the Cloudflare kinds), which also needs a metrics address.
    #[must_use]
    pub fn uses_ready_probe(self) -> bool {
        matches!(self, Self::Quick | Self::Named)
    }

    /// Whether the instance must capture a public URL from its log before it
    /// counts as ready (every kind but the named tunnel).
    #[must_use]
    pub fn needs_url(self) -> bool {
        !matches!(self, Self::Named)
    }

    /// The program name used in crash and timeout messages.
    #[must_use]
    pub fn program(self) -> &'static str {
        match self {
            Self::Quick | Self::Named => "cloudflared",
            Self::Ngrok { .. } => "ngrok",
            Self::Zrok => "zrok",
        }
    }
}
