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
    Config, DomainDelta, DomainsSection, DumpsSection, ExtEntry, GroupsSection, MailSection,
    ParkedSection, PhpSection, Ports, ServiceInstance, ServicesSection, SiteOverride,
    TunnelSection, DEFAULT_DNS_PORT, DEFAULT_DUMP_PORT, DEFAULT_MAIL_PORT, RESERVED_GROUP_NAME,
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
/// user-registered custom extensions, plus the `wp_auto_login`/
/// `wp_auto_login_user` keys inside `[[linked]]` and `[[overrides]]` for
/// `WordPress` one-click admin login. All default when absent, so the v3→v4,
/// v4→v5, v5→v6, v6→v7, v7→v8, v8→v9, and v9→v10 migrations are bare version
/// bumps; each bump exists so an *older* binary rejects a file using the newer
/// field cleanly as [`ConfigError::UnsupportedVersion`] rather than failing on
/// the unknown key.
///
/// v11 added the optional `[domains]` table ([`DomainsSection`]) for per-site
/// routable-domain customisation (multiple domains, subdomains, wildcards, and a
/// changeable primary). It defaults (empty) when absent, so v10→v11 is a bare
/// version bump.
///
/// v12 added the top-level `symlink_protection` scalar
/// ([`Config::symlink_protection`]) for the user-toggleable proxy symlink-escape
/// guard (defaults to on when absent). v13 added the optional `front_controller`
/// key inside `[[linked]]` and `[[overrides]]` for the per-site
/// front-controller-vs-direct-execution toggle (defaults to auto when absent).
/// Both v11→v12 and v12→v13 are bare version bumps.
///
/// v14 added the optional `[[proxies]]` array and `[proxy_rules]` table (both
/// default to empty), so v13→v14 is a bare bump. v15 reworked services for
/// multiple instances - the optional per-instance `site` field and `"{type}:{site}"`
/// ids - and made `enabled` gate boot autostart, so its migration marks every
/// pre-existing single-instance engine `enabled = true` rather than silently
/// stopping engines that used to start.
///
/// v16 added the optional `[php.version_settings]` table
/// ([`PhpSection::version_settings`]) for per-version overrides of the global
/// PHP settings. It defaults (empty) when absent, so v15→v16 is a bare
/// version bump. v17 added the top-level `mcp_enabled` scalar
/// ([`Config::mcp_enabled`]) gating the MCP server for AI agents (defaults to off
/// when absent), also a bare bump. v18 added the optional `[php.directives]`
/// table ([`PhpSection::directives`]) for free-form per-version ini
/// directives; it defaults (empty) when absent, so v17→v18 is a bare bump.
///
/// The per-version detail, including how to hand-edit a file back down for an
/// older binary, lives in `docs/developer/config-schema-history.md`.
pub const CURRENT_VERSION: u32 = 18;
