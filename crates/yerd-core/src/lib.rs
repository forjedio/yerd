//! Pure domain types and host→site routing for Yerd.
//!
//! This crate is the foundation of the Yerd workspace: every other crate
//! depends on it. It is **pure**: no I/O, no async, no internal `yerd-*`
//! dependencies. Side effects belong behind traits in `yerd-platform` and
//! similar adapter crates.

#![forbid(unsafe_code)]

mod error;
mod host;
mod php;
mod router;
mod site;
mod tld;

pub use error::{CoreError, PhpVersionErrorReason, SiteNameErrorReason, TldErrorReason};
pub use php::PhpVersion;
pub use router::{RouterConfig, SiteRouter};
pub use site::{Site, SiteKind};
pub use tld::Tld;
