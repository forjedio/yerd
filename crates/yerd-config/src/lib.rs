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
pub use schema::{
    Config, DumpsSection, ParkedSection, PhpSection, Ports, ServiceInstance, ServicesSection,
    SiteOverride, DEFAULT_DNS_PORT, DEFAULT_DUMP_PORT,
};

/// The on-disk schema version this crate writes. Bumped together with a new
/// entry in `migrate::STEPS`.
///
/// Decoupled from `yerd_ipc::PROTOCOL_VERSION` — the on-disk TOML schema and
/// the IPC wire protocol evolve independently.
///
/// v2 added per-site web roots: `web_subpath` inside `[[linked]]` and
/// `web_root` inside `[[overrides]]`. Both are optional and default when
/// absent, so a v1 file migrates forward by a bare version bump. Because the
/// linked/override wire mirrors are `deny_unknown_fields`, an *older* (v1)
/// binary reading a v2 file that uses those keys is rejected cleanly as
/// [`ConfigError::UnsupportedVersion`] rather than failing mid-parse.
///
/// v3 promoted `[services]` from an `enabled = [...]` array of names to per-
/// service `[services.<id>]` tables ([`ServiceInstance`], carrying version /
/// port / enabled). The v2→v3 migration rewrites the old array — the first
/// *structural* migration step (v0→v1 and v1→v2 are bare version bumps).
///
/// v4 is reserved for the mail-capture feature's `[mail]` table (developed on a
/// sibling branch). v5 added the optional `[dumps]` table ([`DumpsSection`]);
/// both default when absent, so the v3→v4 and v4→v5 migrations are bare version
/// bumps.
pub const CURRENT_VERSION: u32 = 5;
