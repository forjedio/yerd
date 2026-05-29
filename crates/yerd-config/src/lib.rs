//! Persisted TOML configuration for Yerd. Pure parse / validate / serialise
//! plus a thin atomic load / save. Schema-versioned; forward migrations land
//! in `migrate.rs`.
//!
//! ## Purity boundary
//!
//! Every function except [`Config::load`] and [`Config::save`] is pure.
//!
//! ## Schema versioning
//!
//! Every on-disk file MUST carry a top-level `version = N` key. A missing
//! key is a hard error. The version is the single trigger for forward
//! migrations. See [`CURRENT_VERSION`].

#![forbid(unsafe_code)]

mod error;
mod io;
mod migrate;
mod parse;
mod schema;
mod serialize;

pub use error::{ConfigError, MigrationErrorReason, ValidateErrorReason};
pub use schema::{Config, ParkedSection, PhpSection, Ports, ServicesSection, DEFAULT_DNS_PORT};

/// The on-disk schema version this crate writes. Bumped together with a new
/// entry in `migrate::STEPS`.
///
/// Decoupled from `yerd_ipc::PROTOCOL_VERSION` — the on-disk TOML schema and
/// the IPC wire protocol evolve independently.
pub const CURRENT_VERSION: u32 = 1;
