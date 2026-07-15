//! Public schema types.
//!
//! The public types implement neither `Serialize` nor `Deserialize` directly.
//! Round-trip goes through crate-internal wire mirrors in [`crate::parse`]
//! and [`crate::serialize`]. This keeps the public surface free of an
//! accidental serde contract that downstream consumers might rely on.

use std::collections::{BTreeMap, BTreeSet};

use yerd_core::{Domain, PhpVersion, ProxyRule, ProxySite, Site, Tld};

/// Top-level on-disk config.
///
/// `version` is private. All `Config` values produced by this build carry
/// `version == crate::CURRENT_VERSION`; there is no public accessor because
/// it would only ever return that constant. Callers wanting the on-disk
/// version should read [`crate::CURRENT_VERSION`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub(crate) version: u32,
    /// TLD served by Yerd's resolver. Default: `"test"`.
    pub tld: Tld,
    /// Loopback UDP/TCP port for the embedded `.test` DNS responder. Default:
    /// [`DEFAULT_DNS_PORT`]. A fixed port (rather than ephemeral) keeps the
    /// resolver config installed by `yerd elevate resolver` valid across daemon
    /// restarts. `0` means "ephemeral" (dev/tests only - not durable).
    pub dns_port: u16,
    /// Self-update release channel: [`DEFAULT_UPDATE_CHANNEL`] (`"stable"`) or
    /// `"edge"`. `stable` tracks the latest non-pre-release; `edge` opts into
    /// pre-releases / release candidates. Read by `yerd update` and the GUI
    /// Settings selector; validated to one of those two values by
    /// [`Config::validate`]. Stored as a `String` (not a typed enum) to avoid a
    /// dependency on the higher-level `yerd-update` crate.
    pub update_channel: String,
    /// Whether the proxy refuses to serve a static asset or execute a script
    /// that is reached via a symlink resolving outside the site's own document
    /// root. Default: `true` (protection on). Set to `false` to allow such
    /// symlinks - e.g. a shared `WordPress` parent theme kept beside the site and
    /// symlinked into `wp-content/themes/`. Read live by `yerd-proxy` via a
    /// shared atomic; toggling it takes effect without a daemon restart. See
    /// [`DEFAULT_SYMLINK_PROTECTION`].
    pub symlink_protection: bool,
    /// Whether `yerd mcp` serves Yerd's tools to local AI agents over MCP.
    /// Default: `false` (opt in from the GUI's General settings). The daemon
    /// runs no MCP server itself: it only persists this flag and reports it in
    /// [`yerd_ipc::StatusReport`], which each `yerd mcp` session reads to decide
    /// whether to serve. See [`DEFAULT_MCP_ENABLED`].
    pub mcp_enabled: bool,
    /// HTTP / HTTPS listen ports.
    pub ports: Ports,
    /// PHP defaults.
    pub php: PhpSection,
    /// Parked directories.
    pub parked: ParkedSection,
    /// Explicitly linked sites. Order is preserved on round-trip.
    pub linked: Vec<Site>,
    /// Per-site overrides for **parked** sites, keyed by document-root path.
    ///
    /// A parked site is otherwise derived purely from its directory listing, so
    /// it has no persistent record to hold a customised PHP version or HTTPS
    /// flag. Rather than promoting it to a [`Self::linked`] entry (which would
    /// flip its kind), the daemon records the override here and re-applies it
    /// during the directory scan, leaving the site parked. The value struct is
    /// extensible (all-`Option` fields) so future per-site settings slot in
    /// additively.
    ///
    /// The key is the parked site's `document_root` **string, byte-exact and
    /// non-canonical** - see [`SiteOverride`]. `BTreeMap` for stable
    /// serialisation order.
    pub overrides: BTreeMap<String, SiteOverride>,
    /// Optional services.
    pub services: ServicesSection,
    /// Built-in mail-capture SMTP server (Herd-style). Enabled by default.
    pub mail: MailSection,
    /// Dump-telemetry settings (the Laravel ▸ Dumps feature).
    pub dumps: DumpsSection,
    /// Cloudflare Tunnel persistence (Named Tunnels). Empty by default.
    pub tunnel: TunnelSection,
    /// User-defined site groups and per-site membership. Empty by default.
    pub groups: GroupsSection,
    /// Per-site routable-domain customisations (added/suppressed domains and a
    /// chosen primary), split by site class. Empty by default.
    pub domains: DomainsSection,
    /// Whole-host reverse proxies (`reverb.test` → `http(s)://host:port`). Order
    /// is preserved on round-trip. Empty by default.
    pub proxies: Vec<ProxySite>,
    /// Per-site path-prefix reverse-proxy rules (`app.test/app` → upstream),
    /// split by site class. Empty by default.
    pub proxy_rules: ProxyRulesSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: crate::CURRENT_VERSION,
            tld: Tld::default(),
            dns_port: DEFAULT_DNS_PORT,
            update_channel: DEFAULT_UPDATE_CHANNEL.to_owned(),
            symlink_protection: DEFAULT_SYMLINK_PROTECTION,
            mcp_enabled: DEFAULT_MCP_ENABLED,
            ports: Ports::default(),
            php: PhpSection::default(),
            parked: ParkedSection::default(),
            linked: Vec::new(),
            overrides: BTreeMap::new(),
            services: ServicesSection::default(),
            mail: MailSection::default(),
            dumps: DumpsSection::default(),
            tunnel: TunnelSection::default(),
            groups: GroupsSection::default(),
            domains: DomainsSection::default(),
            proxies: Vec::new(),
            proxy_rules: ProxyRulesSection::default(),
        }
    }
}

/// Per-site path-prefix reverse-proxy rules (see [`Config::proxy_rules`]).
///
/// Split by site class exactly like [`DomainsSection`]: **linked** rules key by
/// site name; **parked** rules key by document-root string (byte-exact, never
/// canonicalised - see [`SiteOverride`]). A site with no rules has no entry, so
/// an uncustomised config omits the whole section. The daemon applies these onto
/// [`yerd_core::SiteRouter`] at build time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyRulesSection {
    /// Linked-site rules, keyed by site name. `BTreeMap` for stable order.
    pub linked: BTreeMap<String, Vec<ProxyRule>>,
    /// Parked-site rules, keyed by document-root string. `BTreeMap` for stable
    /// order.
    pub parked: BTreeMap<String, Vec<ProxyRule>>,
}

impl ProxyRulesSection {
    /// True when neither side holds any non-empty rule list - matching the
    /// serialiser, which prunes empty-vector entries, so a section whose only
    /// entries are empty rule lists round-trips as absent (the `[proxy_rules]`
    /// table is omitted and a default config stays byte-stable).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.linked
            .values()
            .chain(self.parked.values())
            .all(Vec::is_empty)
    }
}

/// Per-site routable-domain customisations (see [`Config::domains`]).
///
/// A site answers only its **effective** domain set, computed as
/// `(default apex - suppressed) + added` (see
/// [`yerd_core::effective_domains`]). Only the *delta* from the default is
/// stored, so an uncustomised site has no entry and the whole section is omitted
/// from a default config.
///
/// Split by site class to mirror the existing storage idioms: **linked** sites
/// are name-stable so they key by site name (like [`Config::linked`]); **parked**
/// sites are directory-derived so they key by document-root string (like
/// [`Config::overrides`]), which survives a directory rename without
/// misattributing a routing set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainsSection {
    /// Linked-site deltas, keyed by site name. `BTreeMap` for stable order.
    pub linked: BTreeMap<String, DomainDelta>,
    /// Parked-site deltas, keyed by document-root string (byte-exact, never
    /// canonicalised - see [`SiteOverride`]). `BTreeMap` for stable order.
    pub parked: BTreeMap<String, DomainDelta>,
}

impl DomainsSection {
    /// True when there are no linked and no parked deltas, letting the serialiser
    /// omit the `[domains]` table so a default config stays byte-stable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.linked.is_empty() && self.parked.is_empty()
    }
}

/// One site's routable-domain delta from its default (apex-only) set.
///
/// Extensible/all-defaultable so future per-site domain settings slot in
/// additively. An all-empty delta is equivalent to no entry and should be pruned
/// by the writer (the daemon) rather than persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainDelta {
    /// Domains added on top of the default apex (exact or single-label wildcard).
    pub added: Vec<Domain>,
    /// Default domains (only ever the apex) the user suppressed.
    pub suppressed: Vec<Domain>,
    /// The chosen primary (canonical) domain, or `None` to derive it. Must be an
    /// exact (non-wildcard) domain when set.
    pub primary: Option<Domain>,
}

impl DomainDelta {
    /// True when the delta carries no customisation (equivalent to no entry).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.suppressed.is_empty() && self.primary.is_none()
    }
}

/// User-defined site groups (see [`Config::groups`]).
///
/// Purely an organisational overlay for the GUI's Sites view: groups do not
/// affect routing. Modeled on [`TunnelSection`] - membership is keyed by
/// **site name** (not document-root) so a site's group applies to parked and
/// linked sites alike, without touching the [`Site`] wire shape. Both fields
/// are empty by default, so a config without a `[groups]` table is the common
/// case.
///
/// The synthetic "Unallocated" bucket (sites with no membership) lives only in
/// the GUI and is never stored here; the name `Unallocated` is reserved and
/// rejected by [`Config::validate`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupsSection {
    /// Group display names in display order; the index is the ordering. Names
    /// are arbitrary display strings (deduplicated ASCII-case-insensitively).
    pub order: Vec<String>,
    /// Per-site group membership, by site name → group name. A site maps to at
    /// most one group; an absent key means "Unallocated". `BTreeMap` for stable
    /// serialisation order.
    pub members: BTreeMap<String, String>,
}

impl GroupsSection {
    /// True when there are no groups and no memberships - lets the serialiser
    /// omit the `[groups]` table entirely so a default config stays byte-stable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.members.is_empty()
    }
}

/// The reserved group name for the GUI's synthetic ungrouped bucket. It is never
/// a real stored group, so [`Config::validate`] (and the daemon's create-group
/// mutation) reject it case-insensitively.
pub const RESERVED_GROUP_NAME: &str = "Unallocated";

/// Cloudflare Tunnel persistence (the Named Tunnels feature).
///
/// Both maps are empty by default, so a config without a `[tunnel]` table is the
/// common case. Keyed by **site name** (not document-root) so the per-site
/// hostname applies to parked and linked sites alike.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelSection {
    /// Named tunnels created locally, by tunnel name → tunnel UUID.
    pub named: BTreeMap<String, String>,
    /// Per-site public hostname mapping, by site name → hostname.
    pub sites: BTreeMap<String, String>,
}

/// Default loopback port for the dump server (see [`DumpsSection::port`]).
pub const DEFAULT_DUMP_PORT: u16 = 2304;

/// Dump-telemetry settings.
///
/// The daemon writes a runtime mirror of these to a state file the
/// `yerd-php-ext` extension reads each request; the config here is the durable
/// source of truth. Defaults are off, port [`DEFAULT_DUMP_PORT`], and no
/// per-feature overrides (every feature on when interception is enabled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpsSection {
    /// Whether dump interception is enabled (the "antenna").
    pub enabled: bool,
    /// Loopback port the dump server listens on and the extension connects to.
    pub port: u16,
    /// When `false` (default), the dump buffer is cleared each time a new request
    /// arrives, so the viewer shows only the latest request (pinned events
    /// survive). When `true`, events accumulate across requests.
    pub persist: bool,
    /// Per-feature capture toggles, keyed by feature name
    /// (`dumps`/`queries`/`jobs`/`views`/`requests`/`logs`/`cache`). An absent
    /// key means "on". `BTreeMap` for stable serialisation order.
    pub features: BTreeMap<String, bool>,
}

impl Default for DumpsSection {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_DUMP_PORT,
            persist: false,
            features: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Parse a TOML document. Runs schema-version routing → wire-mirror
    /// deserialisation → `TryFrom<Wire>` (which invokes `yerd-core`'s
    /// per-field validators on `Tld`, `PhpVersion`, and `Site`, surfacing
    /// [`crate::ConfigError::Core`] on failure) → [`Self::validate`].
    pub fn from_toml(s: &str) -> Result<Self, crate::ConfigError> {
        crate::parse::parse_toml(s)
    }

    /// Serialise to TOML. Always writes `version = CURRENT_VERSION`.
    pub fn to_toml(&self) -> Result<String, crate::ConfigError> {
        crate::serialize::to_toml(self)
    }

    /// Validate cross-field invariants, plus per-field invariants that the
    /// storage of `parked.paths` and `services.instances` cannot enforce
    /// structurally (empty strings, unknown service ids).
    /// Per-field invariants on typed fields (TLD, `PhpVersion`, `Site`
    /// name) are enforced earlier, during `Wire` → `Config` conversion.
    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        crate::parse::validate(self)
    }

    /// Thin I/O leaf - read + parse a TOML file at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self, crate::ConfigError> {
        crate::io::load(path)
    }

    /// Thin I/O leaf - serialise + save atomically via write-temp-then-rename.
    ///
    /// `save` may create intermediate parent directories
    /// (`fs::create_dir_all`); they are not removed on a later failure. On
    /// Unix the destination ends up with mode 0600 (owner read/write only)
    /// inherited from the temp file - the daemon is the only intended
    /// writer.
    ///
    /// A parent-less path (e.g. `Path::new("config.toml")`) is treated as
    /// relative to the process's current working directory.
    pub fn save(&self, path: &std::path::Path) -> Result<(), crate::ConfigError> {
        crate::io::save(self, path)
    }
}

/// Default loopback port for the embedded DNS responder (see [`Config::dns_port`]).
pub const DEFAULT_DNS_PORT: u16 = 1053;

/// Default self-update channel (see [`Config::update_channel`]).
pub const DEFAULT_UPDATE_CHANNEL: &str = "stable";

/// The two accepted [`Config::update_channel`] values.
pub const UPDATE_CHANNELS: &[&str] = &["stable", "edge"];

/// Default for [`Config::symlink_protection`] - protection on.
pub const DEFAULT_SYMLINK_PROTECTION: bool = true;

/// Default for [`Config::mcp_enabled`] - off, so exposing Yerd to AI agents is
/// an explicit opt-in.
pub const DEFAULT_MCP_ENABLED: bool = false;

/// The lowest non-privileged port. Ports below this need elevation on
/// macOS / Linux, which is precisely what the rootless fallback exists to
/// avoid - so `fallback_http`/`fallback_https` are required to be `>=` this.
pub const FIRST_UNPRIVILEGED_PORT: u16 = 1024;

/// HTTP and HTTPS ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// HTTP listen port. Default: 80.
    pub http: u16,
    /// HTTPS listen port. Default: 443.
    pub https: u16,
    /// Rootless HTTP port the daemon drops to when `http` can't bind without
    /// elevation. Must be `>= FIRST_UNPRIVILEGED_PORT`. Default: 8080.
    pub fallback_http: u16,
    /// Rootless HTTPS port the daemon drops to when `https` can't bind without
    /// elevation. Must be `>= FIRST_UNPRIVILEGED_PORT`. Default: 8443.
    pub fallback_https: u16,
}

impl Ports {
    /// IANA well-known pair, `80 / 443`, with the `8080 / 8443` rootless
    /// fallback. Default. Binding 80/443 may require elevation on macOS /
    /// Linux; Windows does not gate them.
    #[must_use]
    pub const fn well_known() -> Self {
        Self {
            http: 80,
            https: 443,
            fallback_http: 8080,
            fallback_https: 8443,
        }
    }

    /// Unprivileged pair, `8080 / 8443`, used directly for `http`/`https` (the
    /// fallback fields keep their `8080 / 8443` defaults).
    #[must_use]
    pub const fn unprivileged() -> Self {
        Self {
            http: 8080,
            https: 8443,
            fallback_http: 8080,
            fallback_https: 8443,
        }
    }
}

impl Default for Ports {
    fn default() -> Self {
        Self::well_known()
    }
}

/// PHP defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpSection {
    /// Default PHP version applied to new sites.
    pub default: PhpVersion,
    /// Global PHP ini settings applied to every installed version's FPM pool
    /// (keyed by directive name, e.g. `"memory_limit" -> "512M"`). Validated
    /// against [`yerd_core::php_settings`]; an empty map means "PHP defaults".
    /// `BTreeMap` for stable serialisation order.
    pub settings: BTreeMap<String, String>,
    /// User-registered custom extensions to load, keyed by PHP version and
    /// applied to both that version's FPM pool and its CLI. Empty by default,
    /// so the `[php.extensions]` table is omitted from a default config.
    ///
    /// A native extension's ABI is bound to a PHP *minor*, so an entry only ever
    /// applies to the version it is keyed under. Each entry's path is validated
    /// by [`yerd_core::php_extensions`] (the ini/`-d` injection boundary); the
    /// daemon additionally load-probes it before persisting. `BTreeMap`/`Vec`
    /// keep serialisation order stable.
    pub extensions: BTreeMap<PhpVersion, Vec<ExtEntry>>,
}

/// One registered custom PHP extension (see [`PhpSection::extensions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtEntry {
    /// Stable handle used to remove the entry and to label it in the GUI;
    /// defaults to the `.so` basename when the user does not supply one.
    pub name: String,
    /// Absolute path to the `.so`, stored byte-exact (a `String` for the same
    /// non-UTF-8/portability reason as [`ParkedSection::paths`]).
    pub path: String,
    /// Load as a `zend_extension` (`true`) rather than a plain `extension`.
    pub zend: bool,
}

impl Default for PhpSection {
    fn default() -> Self {
        Self {
            default: PhpVersion::new(8, 3),
            settings: yerd_core::php_settings::default_settings()
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            extensions: BTreeMap::new(),
        }
    }
}

/// Parked directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParkedSection {
    /// Set of parked directory paths, stored verbatim as UTF-8 strings.
    ///
    /// `String` (not `PathBuf`) is intentional: this crate does not own
    /// platform-specific path semantics, and `PathBuf::serialize` is lossy
    /// for non-UTF-8 paths on Windows. Callers convert to `PathBuf` at the
    /// point of use.
    ///
    /// Storage is byte-exact - the config layer does not canonicalise.
    /// `"/srv/foo"` and `"/srv/foo/"` are distinct entries. Callers wanting
    /// equality semantics must normalise before insertion.
    ///
    /// `BTreeSet` so the serialiser yields stable lexicographic order and
    /// duplicates are structurally impossible.
    pub paths: BTreeSet<String>,
}

/// A per-site override applied to a **parked** site (see [`Config::overrides`]).
///
/// Every field is `Option`: `None` means "inherit" (global default PHP, or
/// HTTPS off), `Some(v)` pins that value. This keeps the type extensible -
/// future per-site settings (e.g. a `FrankenPHP` toggle) are added as more
/// `Option` fields without a wire break.
///
/// ## Keying
///
/// In [`Config::overrides`] the key is the parked site's `document_root`
/// **string, stored byte-exact and never canonicalised** - exactly the form
/// produced by `Site::document_root().to_string_lossy()`, which is in turn the
/// `std::fs::DirEntry::path()` the daemon's scan yields. Because both the writer
/// (the IPC mutation handler) and the reader (the directory scan) derive the key
/// from that same `DirEntry::path()` lineage, the strings match without any
/// normalisation. Do **not** canonicalise one side independently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteOverride {
    /// Pinned PHP version, or `None` to inherit the global default.
    pub php: Option<PhpVersion>,
    /// Pinned HTTPS flag, or `None` to inherit (off).
    pub secure: Option<bool>,
    /// Pinned web root, **relative to** the site's `document_root`, or `None`
    /// to auto-detect each scan. Stored as a string for the same byte-stability
    /// reason as the override key. Must be a plain relative path (no leading
    /// `/`, no `..`); enforced by [`Config::validate`].
    pub web_root: Option<String>,
    /// Pinned `WordPress` one-click admin login flag, or `None` to inherit
    /// (off).
    pub wp_auto_login: Option<bool>,
    /// Pinned `WordPress` login/username to sign in as, or `None` to inherit
    /// (fall back to the earliest-created administrator). Distinct from
    /// `wp_auto_login` being absent - the two are independent overrides, but
    /// only meaningful together (a chosen user has no effect while
    /// `wp_auto_login` is off).
    pub wp_auto_login_user: Option<String>,
    /// Pinned front-controller flag (`true` = funnel through `index.php`,
    /// `false` = execute scripts directly), or `None` to auto-derive each scan
    /// from detection. See [`yerd_core::Site::uses_front_controller`].
    pub front_controller: Option<bool>,
}

/// Configured services, keyed by service id.
///
/// **v3 shape.** Earlier versions stored only an `enabled = ["redis"]` array of
/// names; that could not carry a per-service version or port. v3 promotes each
/// to a [`ServiceInstance`] table (`[services.redis]`); the v2→v3 migration in
/// `crate::migrate` rewrites the old array into the new tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServicesSection {
    /// Per-instance configuration, keyed by the instance *wire id*: a type id
    /// (`"redis"`, `"mysql"`, ...) for a single-instance engine, or
    /// `"{type}:{site}"` (`"reverb:blog"`) for a per-site instance. Keys are
    /// validated in `parse.rs` (`validate_known_services`): the type must be
    /// known, a per-site type requires a site suffix, and a single-instance type
    /// forbids one.
    ///
    /// `BTreeMap` so the serialiser yields stable lexicographic table order.
    /// Strings as keys (rather than a typed enum) keep the canonical typed
    /// registry in `yerd-services` (downstream of this crate) and allow
    /// forward-compatibility with experimental services without a release.
    pub instances: BTreeMap<String, ServiceInstance>,
}

/// Per-service configuration (one supervised instance).
///
/// Keyed in [`ServicesSection::instances`] by the instance *wire id*: the type id
/// for a single-instance engine (`"redis"`) or `"{type}:{site}"` for a per-site
/// instance (`"reverb:blog"`). Every settable field is `Option` (so the table
/// stays forward-extensible and omits unset keys on the wire) except `enabled`,
/// which always has a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstance {
    /// The selected installed version (e.g. `"8"` for Valkey). `None` =
    /// "use whatever is installed" (the daemon resolves a concrete version).
    /// Always `None` for a version-less type (app servers like Reverb).
    pub version: Option<String>,
    /// Port override. `None` = the type's default port.
    pub port: Option<u16>,
    /// The linked site name, for a per-site instance (`Some("blog")` for a
    /// `"reverb:blog"` key); `None` for a single-instance engine.
    pub site: Option<String>,
    /// Whether this instance starts with Yerd. The daemon honours this at boot
    /// (see `yerdd::services::auto_start_installed`): single-instance engines
    /// default `true`, per-site app servers default `false`.
    pub enabled: bool,
}

impl Default for ServiceInstance {
    fn default() -> Self {
        Self {
            version: None,
            port: None,
            site: None,
            enabled: true,
        }
    }
}

/// Default loopback port for the built-in mail-capture SMTP server.
pub const DEFAULT_MAIL_PORT: u16 = 2525;

/// Built-in mail-capture SMTP server settings (see [`Config::mail`]).
///
/// A Herd-style capture sink: it accepts mail on a loopback port and stores it
/// for inspection in the GUI. Enabled by default; when enabled the daemon binds
/// [`Self::port`] on `127.0.0.1` (a busy port is non-fatal - the daemon logs and
/// runs with capture not listening).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailSection {
    /// Whether the daemon starts the capture SMTP server on boot.
    pub enabled: bool,
    /// Loopback port the capture server listens on. Default:
    /// [`DEFAULT_MAIL_PORT`].
    pub port: u16,
}

impl Default for MailSection {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_MAIL_PORT,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn default_config_carries_current_version() {
        assert_eq!(Config::default().version, crate::CURRENT_VERSION);
    }

    #[test]
    fn default_ports_is_well_known() {
        assert_eq!(Ports::default(), Ports::well_known());
        assert_eq!(Ports::well_known().http, 80);
        assert_eq!(Ports::well_known().https, 443);
    }

    #[test]
    fn unprivileged_ports_match_documented_fallback() {
        assert_eq!(Ports::unprivileged().http, 8080);
        assert_eq!(Ports::unprivileged().https, 8443);
    }

    #[test]
    fn default_php_section_is_8_3() {
        assert_eq!(PhpSection::default().default, PhpVersion::new(8, 3));
    }

    #[test]
    fn default_parked_and_services_empty() {
        assert!(ParkedSection::default().paths.is_empty());
        assert!(ServicesSection::default().instances.is_empty());
    }

    #[test]
    fn default_service_instance_is_enabled_and_unpinned() {
        let inst = ServiceInstance::default();
        assert!(inst.enabled);
        assert_eq!(inst.version, None);
        assert_eq!(inst.port, None);
    }

    #[test]
    fn default_groups_empty() {
        let g = &Config::default().groups;
        assert!(g.is_empty());
        assert!(g.order.is_empty());
        assert!(g.members.is_empty());
    }

    #[test]
    fn default_overrides_empty() {
        assert!(Config::default().overrides.is_empty());
        assert_eq!(
            SiteOverride::default(),
            SiteOverride {
                php: None,
                secure: None,
                web_root: None,
                wp_auto_login: None,
                wp_auto_login_user: None,
                front_controller: None,
            }
        );
    }
}
