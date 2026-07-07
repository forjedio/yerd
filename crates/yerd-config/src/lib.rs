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
    Config, DumpsSection, ExtEntry, GroupsSection, MailSection, ParkedSection, PhpSection, Ports,
    ServiceInstance, ServicesSection, SiteOverride, TunnelSection, DEFAULT_DNS_PORT,
    DEFAULT_DUMP_PORT, DEFAULT_MAIL_PORT, RESERVED_GROUP_NAME,
};

/// The on-disk schema version this crate writes. Bumped together with a new
/// entry in `migrate::STEPS`.
///
/// Decoupled from `yerd_ipc::PROTOCOL_VERSION` - the on-disk TOML schema and
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
/// port / enabled). The v2→v3 migration rewrites the old array - the first
/// *structural* migration step (v0→v1 and v1→v2 are bare version bumps).
///
/// v4 added the optional `[mail]` section ([`MailSection`]) for the built-in
/// mail-capture SMTP server. v5 added the optional `[dumps]` table
/// ([`DumpsSection`]). v6 added the top-level `update_channel` scalar
/// ([`Config::update_channel`]). v7 added the `[ports] fallback_http`/
/// `fallback_https` keys ([`Ports`]). v8 added the optional `[tunnel]` table
/// ([`TunnelSection`]). v9 added the optional `[groups]` table
/// ([`GroupsSection`]) for the GUI's site grouping overlay. v10 added the
/// optional `[php.extensions]` registry ([`PhpSection::extensions`]) for
/// user-registered custom extensions. All default when absent, so the v3→v4,
/// v4→v5, v5→v6, v6→v7, v7→v8, v8→v9, and v9→v10 migrations are bare version
/// bumps; each bump exists so an *older* binary rejects a file using the newer
/// field cleanly as [`ConfigError::UnsupportedVersion`] rather than failing on
/// the unknown key.
pub const CURRENT_VERSION: u32 = 10;
