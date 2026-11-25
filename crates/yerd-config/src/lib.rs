//! Persisted TOML configuration for Yerd. Pure parse / validate / serialise
//! plus a thin atomic load / save. Schema-versioned; forward migrations land
//! in `migrate.rs`.
//!
//! ## Purity boundary
//!
//! Every function except [`Config::load`], [`Config::save`],
//! [`ProjectFile::load`] and [`ProjectFile::save`] is pure.
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
mod project;
mod schema;
mod serialize;

pub use error::{ConfigError, MigrationErrorReason, ProjectError, ValidateErrorReason};
pub use project::{
    ends_with_tld, extension_satisfied, newest_matching, normalise_module, plan, service_action,
    service_prefix_matches, PlannedAction, PlannedStep, ProjectFile, ServiceAction, SiteSnapshot,
    SyncDelta, ANY_SERVICE_VERSION, PROJECT_FILE_MAX_BYTES, PROJECT_FILE_NAME,
};
pub use schema::{
    Config, DomainDelta, DomainsSection, DumpsSection, ExtEntry, FavouritesSection, GroupsSection,
    MailSection, NetworkSection, NgrokSettings, NodeSection, ParkedClaim, ParkedSection,
    PhpSection, Ports, ResponseHeadersSection, ServiceInstance, ServicesSection, SiteOverride,
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
/// v19 added the top-level `lan_enabled` and `lan_setup_port` scalars
/// ([`Config::lan_enabled`], [`Config::lan_setup_port`]) for LAN exposure (both
/// default when absent), also a bare bump. v20 added the optional
/// `[php.pool]` table ([`PhpSection::pool`]) for per-version FPM pool
/// settings; it defaults (empty) when absent, so v19→v20 is a bare bump too.
/// v21 added the optional `[route_rules]` table ([`Config::route_rules`])
/// holding per-site path-prefix routing rules (prefix → a local target under
/// the served root); it defaults (empty) when absent, so v20→v21 is a bare bump.
/// v22 added the optional `[services.<id>.overrides]` table
/// ([`ServiceInstance::overrides`]) holding free-form configuration overrides
/// for a config-backed engine; it defaults (empty) when absent, so v21→v22 is a
/// bare bump too.
/// v23 added the optional `[domains.proxy]` table ([`Config::domains`]) for
/// whole-host proxy domain deltas; it defaults (empty) when absent, so v22→v23
/// is a bare bump too.
/// v24 added the optional `[tunnel.passwords]` table ([`TunnelSection`]) of
/// per-site share passwords; it defaults (empty) when absent, so v23→v24 is a
/// bare bump too.
/// v25 added the optional `[response_headers]` table
/// ([`Config::response_headers`]) holding per-site custom response headers; it
/// defaults (empty) when absent, so v24→v25 is a bare bump too.
/// v26 added the optional, unseeded `ffi.enable` key to `[php.settings]` and
/// `[php.version_settings]`; it is absent unless set, so v25→v26 is a bare bump
/// too. The bump makes an older binary refuse the file by version rather than
/// failing on an unknown setting name.
///
/// v27 added the optional `[node]` table ([`Config::node`]); v28 added the
/// optional `[network]` outbound-proxy table ([`Config::network`]). Both
/// default when absent, so v26→v27 and v27→v28 are bare bumps too. v29 added
/// the optional `[php.xdebug]` per-version table ([`PhpSection::xdebug`]); it
/// defaults (empty) when absent, so v28→v29 is a bare bump too. v30 added the
/// optional `[favourites]` table ([`Config::favourites`]); it defaults (empty)
/// when absent, so v29→v30 is a bare bump too. v31 added the optional
/// `[parked.claims]` map ([`ParkedSection::claims`]); it defaults (empty) when
/// absent, so v30→v31 is a bare bump too. v32 added `[tunnel] default_provider`
/// and the optional `[tunnel.ngrok]` table ([`TunnelSection`]); both default
/// when absent, so v31→v32 is a bare bump too.
///
/// The per-version detail, including how to hand-edit a file back down for an
/// older binary, lives in `docs/developer/config-schema-history.md`.
pub const CURRENT_VERSION: u32 = 32;
