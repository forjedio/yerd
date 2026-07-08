//! Pure domain types and host→site routing for Yerd.
//!
//! This crate is the foundation of the Yerd workspace: every other crate
//! depends on it. It is **pure**: no I/O, no async, no internal `yerd-*`
//! dependencies. Side effects belong behind traits in `yerd-platform` and
//! similar adapter crates.

#![forbid(unsafe_code)]

pub mod detect;
mod domain;
mod error;
mod host;
mod php;
pub mod php_extensions;
pub mod php_settings;
mod router;
mod site;
mod tld;

/// `Server` header value the proxy stamps on its own (synthetic, non-forwarded)
/// responses. It doubles as the signature the macOS privileged-port redirect
/// probe looks for: confirming a connection to `127.0.0.1:80` reaches *this*
/// daemon's proxy - rather than some other process or a stale `pf` rule holding
/// the port - instead of merely confirming *something* answers.
///
/// It is a cross-crate contract: `yerd-proxy` sets it (`server.rs`) and
/// `yerd-platform`'s redirect probe (`port_redirect.rs`) checks for it.
/// Changing the value means updating both ends.
pub const PROXY_SERVER_ID: &str = "yerd";

/// Subject Common Name of yerd's local development CA.
///
/// A cross-crate contract: `bin/yerdd` stamps it onto the generated CA
/// (`startup.rs`), and `yerd-helper` checks for it before removing a CA from
/// the system trust store - so the privileged helper only ever deletes a cert
/// it can confirm is yerd's, never an unrelated trusted root. Changing the
/// value means re-generating existing users' CAs, so treat it as frozen.
pub const CA_COMMON_NAME: &str = "Yerd Local CA";

pub use detect::{detect, Detection, ProjectSignals};
pub use domain::{choose_primary, effective_domains, Domain};
pub use error::{
    CoreError, DomainErrorReason, PhpVersionErrorReason, SiteNameErrorReason, TldErrorReason,
};
pub use php::PhpVersion;
pub use php_extensions::{ExtError, NameErrorReason, PathErrorReason};
pub use php_settings::{PhpSettingError, ValueErrorReason};
pub use router::{RouterConfig, SiteRouter};
pub use site::{slugify_site_name, Site, SiteKind};
pub use tld::Tld;
