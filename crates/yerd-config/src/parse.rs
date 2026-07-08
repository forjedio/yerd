//! TOML deserialisation, wire mirrors, and cross-field validation.
//!
//! The pipeline uses **raw-typed** wire mirrors and a `TryFrom<Wire>` for
//! [`Config`] conversion. Raw types let `yerd-core` validation failures
//! surface as typed [`ConfigError::Core`] rather than being folded into
//! [`ConfigError::Parse`] via `serde::de::Error::custom`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;

use crate::error::ValidateErrorReason;
use crate::schema::{
    Config, DumpsSection, MailSection, ParkedSection, PhpSection, Ports, ServiceInstance,
    ServicesSection,
};
use crate::ConfigError;

pub(crate) const KNOWN_SERVICES: &[&str] = &["mysql", "mariadb", "postgres", "redis"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u32,
    #[serde(default = "default_tld_str")]
    tld: String,
    #[serde(default = "default_dns_port")]
    dns_port: u16,
    // v6: self-update channel. `default` is mandatory (Wire is
    // `deny_unknown_fields`) so a v1..v5 file with no `update_channel` key still
    // parses, defaulting to "stable".
    #[serde(default = "default_update_channel")]
    update_channel: String,
    // v11: proxy symlink-escape protection. `default` is mandatory (Wire is
    // `deny_unknown_fields`) so a v1..v10 file with no `symlink_protection` key
    // still parses, defaulting to on.
    #[serde(default = "default_symlink_protection")]
    symlink_protection: bool,
    #[serde(default)]
    ports: PortsWire,
    #[serde(default)]
    php: PhpSectionWire,
    #[serde(default)]
    parked: ParkedSectionWire,
    #[serde(default)]
    linked: Vec<SiteWire>,
    // `default` is mandatory: `Wire` is `deny_unknown_fields`, so a v1 config
    // written before overrides existed has no `[[overrides]]` table and must
    // still parse. Empty here ↔ omitted on the wire (serializer skips empty).
    #[serde(default)]
    overrides: Vec<OverrideWire>,
    // v3: per-service tables keyed by service id (`[services.redis]`). A v2
    // `enabled = [...]` array is rewritten into this shape by the v2→v3
    // migration before deserialisation, so this never sees the old array.
    #[serde(default)]
    services: BTreeMap<String, ServiceInstanceWire>,
    // v4: built-in mail-capture server. `default` is mandatory (Wire is
    // `deny_unknown_fields`) so a v1/v2/v3 file with no `[mail]` still parses.
    #[serde(default)]
    mail: MailSectionWire,
    // v5: optional `[dumps]` table; absent in v4 and earlier → default
    // (disabled, port 2304, no per-feature overrides).
    #[serde(default)]
    dumps: DumpsSectionWire,
    // v8: optional `[tunnel]` table; absent in v7 and earlier → default (empty).
    #[serde(default)]
    tunnel: TunnelSectionWire,
    // v9: optional `[groups]` table; absent in v8 and earlier → default (empty).
    #[serde(default)]
    groups: GroupsSectionWire,
}

/// The `[groups]` table. Both fields default to empty, so an absent table parses
/// to [`crate::schema::GroupsSection::default`].
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupsSectionWire {
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    members: BTreeMap<String, String>,
}

/// The `[tunnel]` table. Both maps default to empty, so an absent table parses
/// to [`crate::schema::TunnelSection::default`].
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TunnelSectionWire {
    #[serde(default)]
    named: BTreeMap<String, String>,
    #[serde(default)]
    sites: BTreeMap<String, String>,
}

/// The `[dumps]` table. All fields default, so an absent table parses to
/// [`DumpsSection::default`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DumpsSectionWire {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_dump_port")]
    port: u16,
    #[serde(default)]
    persist: bool,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

impl Default for DumpsSectionWire {
    fn default() -> Self {
        Self {
            enabled: false,
            port: crate::schema::DEFAULT_DUMP_PORT,
            persist: false,
            features: BTreeMap::new(),
        }
    }
}

fn default_dump_port() -> u16 {
    crate::schema::DEFAULT_DUMP_PORT
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortsWire {
    http: u16,
    https: u16,
    // Additive: configs written before rootless ports were configurable omit
    // these, so each carries a field-level default (8080 / 8443).
    #[serde(default = "default_fallback_http")]
    fallback_http: u16,
    #[serde(default = "default_fallback_https")]
    fallback_https: u16,
}

fn default_fallback_http() -> u16 {
    Ports::default().fallback_http
}

fn default_fallback_https() -> u16 {
    Ports::default().fallback_https
}

impl Default for PortsWire {
    fn default() -> Self {
        let p = Ports::default();
        Self {
            http: p.http,
            https: p.https,
            fallback_http: p.fallback_http,
            fallback_https: p.fallback_https,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhpSectionWire {
    default: String,
    #[serde(default)]
    settings: BTreeMap<String, String>,
    // v10: custom extensions keyed by version string. `default` is mandatory
    // (`Wire` is `deny_unknown_fields`) so a pre-v10 file with no
    // `[php.extensions]` still parses. Version keys and entry fields are kept
    // raw here and validated in `TryFrom<Wire>` / `validate`.
    #[serde(default)]
    extensions: BTreeMap<String, Vec<ExtEntryWire>>,
}

impl Default for PhpSectionWire {
    fn default() -> Self {
        Self {
            default: PhpSection::default().default.to_string(),
            settings: BTreeMap::new(),
            extensions: BTreeMap::new(),
        }
    }
}

/// One `[[php.extensions."<ver>"]]` table. `name` is optional on the wire and
/// defaults to the `.so` basename in `TryFrom` when absent (hand-edited configs
/// may omit it).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtEntryWire {
    #[serde(default)]
    name: Option<String>,
    path: String,
    #[serde(default)]
    zend: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ParkedSectionWire {
    #[serde(default)]
    paths: BTreeSet<String>,
}

/// One `[services.<id>]` table. `enabled` defaults to `true` (a configured
/// instance is on unless explicitly disabled); `version`/`port` are unset until
/// pinned.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceInstanceWire {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default = "default_service_enabled")]
    enabled: bool,
}

fn default_service_enabled() -> bool {
    true
}

/// The `[mail]` table. Both keys default (off / 2525) so a config written before
/// v4 - which has no `[mail]` table at all - still deserialises.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSectionWire {
    #[serde(default = "default_mail_enabled")]
    enabled: bool,
    #[serde(default = "default_mail_port")]
    port: u16,
}

impl Default for MailSectionWire {
    fn default() -> Self {
        let m = MailSection::default();
        Self {
            enabled: m.enabled,
            port: m.port,
        }
    }
}

fn default_mail_enabled() -> bool {
    MailSection::default().enabled
}

fn default_mail_port() -> u16 {
    MailSection::default().port
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteWire {
    name: String,
    document_root: PathBuf,
    // Optional; absent in v1 `[[linked]]` tables → empty (serve document root).
    #[serde(default)]
    web_subpath: PathBuf,
    php: String,
    secure: bool,
    kind: yerd_core::SiteKind,
    // Optional; absent in configs written before this field existed.
    #[serde(default)]
    wp_auto_login: bool,
    #[serde(default)]
    wp_auto_login_user: Option<String>,
}

/// One `[[overrides]]` table: a parked site's document-root `path` plus the
/// optional values to pin. `php` is kept raw (`Option<String>`) so a bad
/// version surfaces as [`ConfigError::Core`] via `PhpVersion::from_str` in
/// `TryFrom`, not a serde custom error.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OverrideWire {
    path: String,
    #[serde(default)]
    php: Option<String>,
    #[serde(default)]
    secure: Option<bool>,
    #[serde(default)]
    web_root: Option<String>,
    #[serde(default)]
    wp_auto_login: Option<bool>,
    #[serde(default)]
    wp_auto_login_user: Option<String>,
}

fn default_tld_str() -> String {
    yerd_core::Tld::default().as_str().to_owned()
}

fn default_dns_port() -> u16 {
    crate::schema::DEFAULT_DNS_PORT
}

fn default_update_channel() -> String {
    crate::schema::DEFAULT_UPDATE_CHANNEL.to_owned()
}

fn default_symlink_protection() -> bool {
    crate::schema::DEFAULT_SYMLINK_PROTECTION
}

pub(crate) fn parse_toml(s: &str) -> Result<Config, ConfigError> {
    let mut value: toml::Value = toml::from_str(s)?;
    let found = crate::migrate::read_version(&value)?;
    if found > crate::CURRENT_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found,
            current: crate::CURRENT_VERSION,
        });
    }
    if found < crate::CURRENT_VERSION {
        crate::migrate::up(&mut value, found)?;
    }
    let wire: Wire = value.try_into()?;
    let cfg = Config::try_from(wire)?;
    validate(&cfg)?;
    Ok(cfg)
}

impl TryFrom<Wire> for Config {
    type Error = ConfigError;

    #[allow(clippy::too_many_lines)]
    fn try_from(w: Wire) -> Result<Self, ConfigError> {
        if w.version != crate::CURRENT_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: w.version,
                current: crate::CURRENT_VERSION,
            });
        }
        let tld = yerd_core::Tld::new(&w.tld)?;
        let php = PhpSection {
            default: yerd_core::PhpVersion::from_str(&w.php.default)?,
            settings: w.php.settings,
            extensions: convert_extensions(w.php.extensions)?,
        };
        let ports = Ports {
            http: w.ports.http,
            https: w.ports.https,
            fallback_http: w.ports.fallback_http,
            fallback_https: w.ports.fallback_https,
        };
        let parked = ParkedSection {
            paths: w.parked.paths,
        };
        let mut overrides = BTreeMap::new();
        for o in w.overrides {
            let php = o
                .php
                .map(|s| yerd_core::PhpVersion::from_str(&s))
                .transpose()?;
            overrides.insert(
                o.path,
                crate::schema::SiteOverride {
                    php,
                    secure: o.secure,
                    web_root: o.web_root,
                    wp_auto_login: o.wp_auto_login,
                    wp_auto_login_user: o.wp_auto_login_user,
                },
            );
        }
        let services = ServicesSection {
            instances: w
                .services
                .into_iter()
                .map(|(name, inst)| {
                    (
                        name,
                        ServiceInstance {
                            version: inst.version,
                            port: inst.port,
                            enabled: inst.enabled,
                        },
                    )
                })
                .collect(),
        };
        let linked = convert_linked(w.linked)?;
        let mail = MailSection {
            enabled: w.mail.enabled,
            port: w.mail.port,
        };
        let dumps = DumpsSection {
            enabled: w.dumps.enabled,
            port: w.dumps.port,
            persist: w.dumps.persist,
            features: w.dumps.features,
        };
        let tunnel = crate::schema::TunnelSection {
            named: w.tunnel.named,
            sites: w.tunnel.sites,
        };
        let groups = crate::schema::GroupsSection {
            order: w.groups.order,
            members: w.groups.members,
        };
        Ok(Config {
            version: crate::CURRENT_VERSION,
            tld,
            dns_port: w.dns_port,
            update_channel: w.update_channel,
            symlink_protection: w.symlink_protection,
            ports,
            php,
            parked,
            linked,
            overrides,
            services,
            mail,
            dumps,
            tunnel,
            groups,
        })
    }
}

/// Rebuild the `linked` site list from its wire mirror, surfacing a bad
/// `PhpVersion` or `Site` name as [`ConfigError::Core`].
fn convert_linked(wire: Vec<SiteWire>) -> Result<Vec<yerd_core::Site>, ConfigError> {
    let mut linked = Vec::with_capacity(wire.len());
    for sw in wire {
        let php_v = yerd_core::PhpVersion::from_str(&sw.php)?;
        let mut s = match sw.kind {
            yerd_core::SiteKind::Linked => {
                yerd_core::Site::linked(&sw.name, sw.document_root, php_v)?
            }
            yerd_core::SiteKind::Parked => {
                yerd_core::Site::parked(&sw.name, sw.document_root, php_v)?
            }
        };
        s.set_secure(sw.secure);
        s.set_web_subpath(sw.web_subpath);
        s.set_wp_auto_login(sw.wp_auto_login);
        s.set_wp_auto_login_user(sw.wp_auto_login_user);
        linked.push(s);
    }
    Ok(linked)
}

/// Convert the raw wire extensions map (string version keys, optional names)
/// into the typed [`PhpSection::extensions`] shape. A bad version key surfaces as
/// [`ConfigError::Core`] via `PhpVersion::from_str`; an absent name defaults to
/// the `.so` basename.
fn convert_extensions(
    wire: BTreeMap<String, Vec<ExtEntryWire>>,
) -> Result<BTreeMap<yerd_core::PhpVersion, Vec<crate::schema::ExtEntry>>, ConfigError> {
    let mut out = BTreeMap::new();
    for (ver, entries) in wire {
        let v = yerd_core::PhpVersion::from_str(&ver)?;
        let converted = entries
            .into_iter()
            .map(|e| {
                let name = e
                    .name
                    .or_else(|| yerd_core::php_extensions::default_name_from_path(&e.path))
                    .unwrap_or_default();
                crate::schema::ExtEntry {
                    name,
                    path: e.path,
                    zend: e.zend,
                }
            })
            .collect();
        out.insert(v, converted);
    }
    Ok(out)
}

pub(crate) fn validate(c: &Config) -> Result<(), ConfigError> {
    validate_ports(c)?;
    validate_unique_linked(c)?;
    validate_nonempty_paths(c)?;
    validate_web_roots(c)?;
    validate_known_services(c)?;
    validate_php_settings(c)?;
    validate_php_extensions(c)?;
    validate_update_channel(c)?;
    validate_tunnel(c)?;
    validate_groups(c)?;
    Ok(())
}

/// Every `[php.extensions]` entry must have a name and path that pass the pure
/// `yerd_core::php_extensions` boundary (absolute, `.so`, no ini/`-d` injection
/// characters), and names must be unique within a version (the name is the
/// remove handle).
fn validate_php_extensions(c: &Config) -> Result<(), ConfigError> {
    for entries in c.php.extensions.values() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for e in entries {
            if yerd_core::php_extensions::validate_entry(&e.name, &e.path, e.zend).is_err() {
                return Err(ve(ValidateErrorReason::InvalidPhpExtension));
            }
            if !seen.insert(e.name.as_str()) {
                return Err(ve(ValidateErrorReason::DuplicateExtensionName));
            }
        }
    }
    Ok(())
}

/// `[groups]` invariants: every group name in `order` is non-empty, not the
/// reserved `Unallocated` (ASCII-case-insensitive - that name is the GUI's
/// synthetic ungrouped bucket), and unique ASCII-case-insensitively; every
/// `members` value references a group present in `order`. Group identity is
/// ASCII-case-insensitive throughout (matching the daemon's create/delete/assign
/// mutations), so the membership check folds case too - otherwise a hand-edited
/// `order = ["Blog"]` with `members.api = "blog"` would fail-closed the whole
/// config load over a purely cosmetic casing mismatch. Group names are arbitrary
/// display strings and never touch the filesystem, so - unlike `[tunnel]` keys -
/// the charset is intentionally unrestricted beyond non-empty. Whether a keyed
/// site actually exists is not checked: parked sites are discovered from disk and
/// have no config record.
fn validate_groups(c: &Config) -> Result<(), ConfigError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for name in &c.groups.order {
        if name.is_empty() {
            return Err(ve(ValidateErrorReason::GroupNameEmpty));
        }
        if name.eq_ignore_ascii_case(crate::schema::RESERVED_GROUP_NAME) {
            return Err(ve(ValidateErrorReason::GroupNameReserved));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(ve(ValidateErrorReason::GroupDuplicate));
        }
    }
    for group in c.groups.members.values() {
        if !seen.contains(&group.to_ascii_lowercase()) {
            return Err(ve(ValidateErrorReason::GroupMemberDangling));
        }
    }
    Ok(())
}

/// `[tunnel]` entries must have non-empty keys/values, the keys (tunnel names,
/// site names) and UUIDs must be free of path-/YAML-unsafe characters, and every
/// per-site hostname must look like a DNS name (no whitespace, contains a dot).
/// Whether the keyed site actually exists is not checked here: parked sites are
/// discovered from disk and have no config record.
///
/// Two cardinality invariants the daemon relies on are also enforced, so a
/// hand-edited config can't load into a state the runtime silently mishandles:
/// at most one `[tunnel.named]` entry (the daemon runs a single consolidated
/// tunnel and starts only the first), and unique `[tunnel.sites]` hostnames (one
/// ingress rule is emitted per pair, so a duplicate hostname would shadow all
/// but the first site).
fn validate_tunnel(c: &Config) -> Result<(), ConfigError> {
    if c.tunnel.named.len() > 1 {
        return Err(ve(ValidateErrorReason::TunnelMultipleNamed));
    }
    for (name, uuid) in &c.tunnel.named {
        if name.is_empty() || uuid.is_empty() {
            return Err(ve(ValidateErrorReason::TunnelEntryEmpty));
        }
        if !is_safe_key(name) || !is_safe_key(uuid) {
            return Err(ve(ValidateErrorReason::TunnelKeyInvalid));
        }
    }
    let mut seen_hostnames = std::collections::BTreeSet::new();
    for (site, hostname) in &c.tunnel.sites {
        if site.is_empty() || hostname.is_empty() {
            return Err(ve(ValidateErrorReason::TunnelEntryEmpty));
        }
        if !is_safe_key(site) {
            return Err(ve(ValidateErrorReason::TunnelKeyInvalid));
        }
        if !is_plausible_hostname(hostname) {
            return Err(ve(ValidateErrorReason::TunnelHostnameInvalid));
        }
        if !seen_hostnames.insert(hostname.as_str()) {
            return Err(ve(ValidateErrorReason::TunnelDuplicateHostname));
        }
    }
    Ok(())
}

/// A `[tunnel]` map key or UUID is safe when it is a short token of DNS-label-ish
/// characters: it can never act as a path separator, escape `creds/`, or break
/// out of its line in the generated `config.yml`.
fn is_safe_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// A hostname sanity check: a dotted name (at least two labels) where each label
/// is 1..=63 hostname characters and neither starts nor ends with a hyphen, with
/// a total length cap. Cloudflare is the real authority on the name; this catches
/// obvious junk (empty labels like `a..b`, leading-hyphen labels, overlong
/// names) before it reaches `config.yml`.
fn is_plausible_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let mut labels = 0usize;
    for label in host.split('.') {
        labels += 1;
        if !is_hostname_label(label) {
            return false;
        }
    }
    labels >= 2
}

/// One DNS label: non-empty, at most 63 bytes, only alphanumerics and hyphens,
/// and not hyphen-bounded.
fn is_hostname_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Checked last: `update_channel` must be one of [`crate::schema::UPDATE_CHANNELS`]
/// (`"stable"` / `"edge"`). A hand-edited or stale value is rejected here rather
/// than silently coerced.
fn validate_update_channel(c: &Config) -> Result<(), ConfigError> {
    if !crate::schema::UPDATE_CHANNELS.contains(&c.update_channel.as_str()) {
        return Err(ve(ValidateErrorReason::InvalidUpdateChannel));
    }
    Ok(())
}

fn validate_ports(c: &Config) -> Result<(), ConfigError> {
    if c.ports.http == 0 {
        return Err(ve(ValidateErrorReason::HttpPortZero));
    }
    if c.ports.https == 0 {
        return Err(ve(ValidateErrorReason::HttpsPortZero));
    }
    if c.ports.http == c.ports.https {
        return Err(ve(ValidateErrorReason::HttpHttpsPortsEqual));
    }
    if c.ports.fallback_http < crate::schema::FIRST_UNPRIVILEGED_PORT
        || c.ports.fallback_https < crate::schema::FIRST_UNPRIVILEGED_PORT
    {
        return Err(ve(ValidateErrorReason::FallbackPortPrivileged));
    }
    if c.ports.fallback_http == c.ports.fallback_https {
        return Err(ve(ValidateErrorReason::FallbackPortsEqual));
    }
    if c.mail.port == 0 {
        return Err(ve(ValidateErrorReason::MailPortZero));
    }
    if c.dumps.port == 0 {
        return Err(ve(ValidateErrorReason::DumpsPortZero));
    }
    // dns_port == 0 is allowed: 0 means ephemeral and must round-trip
    // (toml_byte_shape::dns_port_zero_round_trips); the zero-port guard
    // lives in the daemon's set_dns_port handler.
    Ok(())
}

fn validate_unique_linked(c: &Config) -> Result<(), ConfigError> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for s in &c.linked {
        if !seen.insert(s.name()) {
            return Err(ve(ValidateErrorReason::DuplicateLinkedSite));
        }
    }
    Ok(())
}

fn validate_nonempty_paths(c: &Config) -> Result<(), ConfigError> {
    for p in &c.parked.paths {
        if p.is_empty() {
            return Err(ve(ValidateErrorReason::ParkedPathEmpty));
        }
    }
    for key in c.overrides.keys() {
        if key.is_empty() {
            return Err(ve(ValidateErrorReason::OverridePathEmpty));
        }
    }
    Ok(())
}

/// Web roots must be plain relative paths so they can only ever resolve to a
/// descendant of the document root (defence against hand-edited absolute or
/// `..`-bearing values; `Site::served_root` is the runtime backstop).
fn validate_web_roots(c: &Config) -> Result<(), ConfigError> {
    for s in &c.linked {
        if web_root_escapes(s.web_subpath()) {
            return Err(ve(ValidateErrorReason::WebRootEscapes));
        }
    }
    for ov in c.overrides.values() {
        if let Some(w) = &ov.web_root {
            if web_root_escapes(std::path::Path::new(w)) {
                return Err(ve(ValidateErrorReason::WebRootEscapes));
            }
        }
    }
    Ok(())
}

fn validate_known_services(c: &Config) -> Result<(), ConfigError> {
    for name in c.services.instances.keys() {
        if !KNOWN_SERVICES.contains(&name.as_str()) {
            return Err(ve(ValidateErrorReason::UnknownService));
        }
    }
    Ok(())
}

/// Checked last (newest invariant): every `php.settings` entry must be a
/// supported directive with a value passing the security/shape validation.
fn validate_php_settings(c: &Config) -> Result<(), ConfigError> {
    for (k, v) in &c.php.settings {
        if yerd_core::php_settings::validate_value(k, v).is_err() {
            return Err(ve(ValidateErrorReason::InvalidPhpSetting));
        }
    }
    Ok(())
}

fn ve(reason: ValidateErrorReason) -> ConfigError {
    ConfigError::Validate { reason }
}

/// True if a web-root subpath could resolve outside its document root: any
/// component that is not a plain name or `.` (i.e. a root, drive/UNC prefix, or
/// `..`). An empty path (serve the document root) is fine.
fn web_root_escapes(p: &std::path::Path) -> bool {
    use std::path::Component;
    p.components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
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
    use crate::error::MigrationErrorReason;

    // ------------------ parse_toml tests ------------------

    #[test]
    fn parse_default_toml_round_trips() {
        let s = Config::default().to_toml().unwrap();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, Config::default());
    }

    #[test]
    fn parse_rejects_missing_version() {
        match Config::from_toml("tld = \"test\"\n") {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::MissingVersion,
            }) => {}
            other => panic!("expected MissingVersion, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_non_integer_version() {
        match Config::from_toml("version = \"1\"\n") {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::NonIntegerVersion,
            }) => {}
            other => panic!("expected NonIntegerVersion, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_negative_version() {
        match Config::from_toml("version = -1\n") {
            Err(ConfigError::Migration {
                reason: MigrationErrorReason::NonIntegerVersion,
            }) => {}
            other => panic!("expected NonIntegerVersion for negative, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_future_version() {
        match Config::from_toml("version = 99\n") {
            Err(ConfigError::UnsupportedVersion {
                found: 99,
                current: 11,
            }) => {}
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_unknown_top_level_key() {
        let s = "version = 1\nbogus = true\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn tunnel_section_parses_and_validates() {
        let s = "version = 8\n[tunnel.named]\nmysite = \"uuid-1\"\n\
                 [tunnel.sites]\napp = \"app.example.com\"\n";
        let c = Config::from_toml(s).unwrap();
        assert_eq!(
            c.tunnel.named.get("mysite").map(String::as_str),
            Some("uuid-1")
        );
        assert_eq!(
            c.tunnel.sites.get("app").map(String::as_str),
            Some("app.example.com")
        );
    }

    #[test]
    fn tunnel_rejects_non_hostname_and_empty_entries() {
        assert!(Config::from_toml("version = 8\n[tunnel.sites]\napp = \"nodot\"\n").is_err());
        assert!(Config::from_toml("version = 8\n[tunnel.sites]\napp = \"\"\n").is_err());
        assert!(Config::from_toml("version = 8\n[tunnel.named]\nmysite = \"\"\n").is_err());
    }

    #[test]
    fn tunnel_rejects_unsafe_keys_and_uuids() {
        let bad_site = "version = 8\n[tunnel.sites]\n\"../escape\" = \"app.example.com\"\n";
        assert!(matches!(
            Config::from_toml(bad_site),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::TunnelKeyInvalid,
            })
        ));
        let bad_uuid = "version = 8\n[tunnel.named]\nmysite = \"../../etc\"\n";
        assert!(matches!(
            Config::from_toml(bad_uuid),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::TunnelKeyInvalid,
            })
        ));
        let bad_name = "version = 8\n[tunnel.named]\n\"a/b\" = \"uuid-1\"\n";
        assert!(matches!(
            Config::from_toml(bad_name),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::TunnelKeyInvalid,
            })
        ));
    }

    #[test]
    fn tunnel_rejects_more_than_one_named_tunnel() {
        let two = "version = 8\n[tunnel.named]\none = \"uuid-1\"\ntwo = \"uuid-2\"\n";
        assert!(matches!(
            Config::from_toml(two),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::TunnelMultipleNamed,
            })
        ));
        let one = "version = 8\n[tunnel.named]\none = \"uuid-1\"\n";
        assert!(Config::from_toml(one).is_ok());
    }

    #[test]
    fn tunnel_rejects_duplicate_site_hostnames() {
        let dup = "version = 8\n[tunnel.sites]\n\
                   app = \"shared.example.com\"\nblog = \"shared.example.com\"\n";
        assert!(matches!(
            Config::from_toml(dup),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::TunnelDuplicateHostname,
            })
        ));
        let unique = "version = 8\n[tunnel.sites]\n\
                      app = \"app.example.com\"\nblog = \"blog.example.com\"\n";
        assert!(Config::from_toml(unique).is_ok());
    }

    #[test]
    fn groups_section_parses_and_round_trips() {
        let s = "version = 10\n[groups]\norder = [\"Blog\", \"Shop\"]\n\
                 [groups.members]\napi = \"Blog\"\n";
        let c = Config::from_toml(s).unwrap();
        assert_eq!(c.groups.order, vec!["Blog".to_string(), "Shop".to_string()]);
        assert_eq!(
            c.groups.members.get("api").map(String::as_str),
            Some("Blog")
        );
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn groups_absent_table_is_empty_and_migrates() {
        let c = Config::from_toml("version = 8\n").unwrap();
        assert!(c.groups.is_empty());
    }

    #[test]
    fn symlink_protection_absent_defaults_on_and_migrates() {
        let c = Config::from_toml("version = 10\n").unwrap();
        assert!(c.symlink_protection);
    }

    #[test]
    fn symlink_protection_false_parses_and_round_trips() {
        let s = "version = 11\nsymlink_protection = false\n";
        let c = Config::from_toml(s).unwrap();
        assert!(!c.symlink_protection);
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn validate_rejects_empty_group_name() {
        let mut c = Config::default();
        c.groups.order.push(String::new());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::GroupNameEmpty,
            }) => {}
            other => panic!("expected GroupNameEmpty, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_reserved_group_name() {
        for name in ["Unallocated", "unallocated", "UNALLOCATED"] {
            let mut c = Config::default();
            c.groups.order.push(name.to_string());
            match c.validate() {
                Err(ConfigError::Validate {
                    reason: ValidateErrorReason::GroupNameReserved,
                }) => {}
                other => panic!("expected GroupNameReserved for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_rejects_case_insensitive_duplicate_group() {
        let mut c = Config::default();
        c.groups.order.push("Blog".to_string());
        c.groups.order.push("blog".to_string());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::GroupDuplicate,
            }) => {}
            other => panic!("expected GroupDuplicate, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_dangling_group_membership() {
        let mut c = Config::default();
        c.groups.order.push("Blog".to_string());
        c.groups
            .members
            .insert("api".to_string(), "Nope".to_string());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::GroupMemberDangling,
            }) => {}
            other => panic!("expected GroupMemberDangling, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_valid_groups() {
        let mut c = Config::default();
        c.groups.order.push("Blog".to_string());
        c.groups.order.push("Shop".to_string());
        c.groups
            .members
            .insert("api".to_string(), "Blog".to_string());
        c.validate().unwrap();
    }

    #[test]
    fn validate_accepts_case_insensitive_group_membership() {
        // A hand-edited casing mismatch between order and members must not
        // fail-closed the whole config load; group identity is case-insensitive.
        let mut c = Config::default();
        c.groups.order.push("Blog".to_string());
        c.groups
            .members
            .insert("api".to_string(), "blog".to_string());
        c.validate().unwrap();
    }

    #[test]
    fn is_plausible_hostname_checks() {
        assert!(is_plausible_hostname("app.example.com"));
        assert!(is_plausible_hostname("a.b"));
        assert!(is_plausible_hostname("a-b.example.com"));
        assert!(!is_plausible_hostname("nodot"));
        assert!(!is_plausible_hostname(".leading"));
        assert!(!is_plausible_hostname("trailing."));
        assert!(!is_plausible_hostname("has space.com"));
        assert!(!is_plausible_hostname("a..b"));
        assert!(!is_plausible_hostname("-app.com"));
        assert!(!is_plausible_hostname("app-.com"));
        assert!(!is_plausible_hostname(&format!("{}.com", "a".repeat(64))));
        assert!(!is_plausible_hostname(&format!("{}.com", "a".repeat(252))));
    }

    #[test]
    fn parse_rejects_unknown_key_under_ports() {
        let s = "version = 1\n[ports]\nhttp = 80\nhttps = 443\nbogus = 0\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_rejects_unknown_key_under_php() {
        let s = "version = 1\n[php]\ndefault = \"8.3\"\nbogus = 0\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_rejects_unknown_key_under_parked() {
        let s = "version = 1\n[parked]\npaths = []\nbogus = 0\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_rejects_unknown_key_under_services() {
        let s = "version = 1\n[services]\nenabled = []\nbogus = 0\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_rejects_unknown_key_under_linked_site() {
        let s = r#"
version = 1
[[linked]]
name = "api"
document_root = "docroot"
php = "8.3"
secure = false
kind = "linked"
bogus = 0
"#;
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_rejects_php_as_bare_scalar() {
        let s = "version = 1\nphp = \"8.3\"\n";
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_accepts_inline_array_of_tables_for_linked_by_value_equality() {
        let inline = r#"
version = 1
linked = [{ name = "api", document_root = "docroot", php = "8.3", secure = false, kind = "linked" }]
"#;
        let header = r#"
version = 1
[[linked]]
name = "api"
document_root = "docroot"
php = "8.3"
secure = false
kind = "linked"
"#;
        let a = Config::from_toml(inline).unwrap();
        let b = Config::from_toml(header).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_propagates_php_version_minor_out_of_range() {
        let s = "version = 1\n[php]\ndefault = \"9.999\"\n";
        match Config::from_toml(s) {
            Err(ConfigError::Core(yerd_core::CoreError::InvalidPhpVersion {
                reason: yerd_core::PhpVersionErrorReason::MinorOutOfRange,
                ..
            })) => {}
            other => panic!("expected MinorOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn parse_propagates_php_version_non_numeric_overflow() {
        let s = "version = 1\n[php]\ndefault = \"8.99999\"\n";
        match Config::from_toml(s) {
            Err(ConfigError::Core(yerd_core::CoreError::InvalidPhpVersion {
                reason: yerd_core::PhpVersionErrorReason::NonNumeric,
                ..
            })) => {}
            other => panic!("expected NonNumeric overflow, got {other:?}"),
        }
    }

    #[test]
    fn parse_propagates_invalid_tld() {
        let s = "version = 1\ntld = \"te st\"\n";
        match Config::from_toml(s) {
            Err(ConfigError::Core(yerd_core::CoreError::InvalidTld {
                reason: yerd_core::TldErrorReason::ContainsWhitespace,
                ..
            })) => {}
            other => panic!("expected ContainsWhitespace, got {other:?}"),
        }
    }

    #[test]
    fn parse_propagates_invalid_site_name() {
        let s = r#"
version = 1
[[linked]]
name = "FOO.BAR"
document_root = "docroot"
php = "8.3"
secure = false
kind = "linked"
"#;
        match Config::from_toml(s) {
            Err(ConfigError::Core(yerd_core::CoreError::InvalidSiteName { .. })) => {}
            other => panic!("expected InvalidSiteName, got {other:?}"),
        }
    }

    #[test]
    fn parse_strips_trailing_dot_from_tld_silently() {
        let s = "version = 1\ntld = \"test.\"\n";
        let c = Config::from_toml(s).unwrap();
        assert_eq!(c.tld.as_str(), "test");
    }

    #[test]
    fn parse_treats_absent_parked_block_as_empty() {
        let c = Config::from_toml("version = 1\n").unwrap();
        assert!(c.parked.paths.is_empty());
    }

    #[test]
    fn parse_treats_absent_services_block_as_empty() {
        let c = Config::from_toml("version = 1\n").unwrap();
        assert!(c.services.instances.is_empty());
    }

    #[test]
    fn parse_treats_absent_overrides_block_as_empty() {
        let c = Config::from_toml("version = 1\n").unwrap();
        assert!(c.overrides.is_empty());
    }

    #[test]
    fn parse_rejects_unknown_key_under_override() {
        let s = r#"
version = 1
[[overrides]]
path = "/srv/blog"
php = "8.4"
bogus = 0
"#;
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_overrides_round_trip() {
        let s = r#"
version = 1
[[overrides]]
path = "/srv/blog"
php = "8.4"
secure = true

[[overrides]]
path = "/srv/wiki"
secure = false
"#;
        let c = Config::from_toml(s).unwrap();
        let blog = c.overrides.get("/srv/blog").unwrap();
        assert_eq!(blog.php, Some(yerd_core::PhpVersion::new(8, 4)));
        assert_eq!(blog.secure, Some(true));
        let wiki = c.overrides.get("/srv/wiki").unwrap();
        assert_eq!(wiki.php, None);
        assert_eq!(wiki.secure, Some(false));
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn parse_propagates_invalid_override_php_version() {
        let s = r#"
version = 1
[[overrides]]
path = "/srv/blog"
php = "not-a-version"
"#;
        assert!(matches!(Config::from_toml(s), Err(ConfigError::Core(_))));
    }

    #[test]
    fn parse_absent_update_channel_defaults_to_stable() {
        let c = Config::from_toml("version = 5\n").unwrap();
        assert_eq!(c.update_channel, "stable");
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn update_channel_round_trips() {
        let mut c = Config::default();
        c.update_channel = "edge".to_string();
        let s = c.to_toml().unwrap();
        assert!(
            s.contains("update_channel = \"edge\""),
            "expected update_channel scalar; got: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back.update_channel, "edge");
        assert_eq!(back, c);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn validate_rejects_unknown_update_channel() {
        let mut c = Config::default();
        c.update_channel = "nightly".to_string();
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::InvalidUpdateChannel,
            }) => {}
            other => panic!("expected InvalidUpdateChannel, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn validate_accepts_each_known_update_channel() {
        for ch in crate::schema::UPDATE_CHANNELS {
            let mut c = Config::default();
            c.update_channel = (*ch).to_string();
            c.validate()
                .unwrap_or_else(|e| panic!("rejected {ch}: {e}"));
        }
    }

    #[test]
    fn validate_rejects_empty_override_path() {
        let mut c = Config::default();
        c.overrides
            .insert(String::new(), crate::SiteOverride::default());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::OverridePathEmpty,
            }) => {}
            other => panic!("expected OverridePathEmpty, got {other:?}"),
        }
    }

    // ------------------ validate tests ------------------

    #[test]
    fn validate_accepts_default() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn validate_rejects_http_zero() {
        let mut c = Config::default();
        c.ports.http = 0;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::HttpPortZero,
            }) => {}
            other => panic!("expected HttpPortZero, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_https_zero() {
        let mut c = Config::default();
        c.ports.https = 0;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::HttpsPortZero,
            }) => {}
            other => panic!("expected HttpsPortZero, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_privileged_fallback_port() {
        let mut c = Config::default();
        c.ports.fallback_http = 80;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::FallbackPortPrivileged,
            }) => {}
            other => panic!("expected FallbackPortPrivileged, got {other:?}"),
        }
        let mut c = Config::default();
        c.ports.fallback_https = 443;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::FallbackPortPrivileged,
            }) => {}
            other => panic!("expected FallbackPortPrivileged, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_1024_fallback_boundary() {
        let mut c = Config::default();
        c.ports.fallback_http = 1024;
        c.ports.fallback_https = 1025;
        c.validate().unwrap();
    }

    #[test]
    fn validate_rejects_equal_fallback_ports() {
        let mut c = Config::default();
        c.ports.fallback_http = 9000;
        c.ports.fallback_https = 9000;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::FallbackPortsEqual,
            }) => {}
            other => panic!("expected FallbackPortsEqual, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_zero_mail_port() {
        let mut c = Config::default();
        c.mail.port = 0;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::MailPortZero,
            }) => {}
            other => panic!("expected MailPortZero, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_zero_dumps_port() {
        let mut c = Config::default();
        c.dumps.port = 0;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::DumpsPortZero,
            }) => {}
            other => panic!("expected DumpsPortZero, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_equal_http_https() {
        let mut c = Config::default();
        c.ports.http = 8000;
        c.ports.https = 8000;
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::HttpHttpsPortsEqual,
            }) => {}
            other => panic!("expected HttpHttpsPortsEqual, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_duplicate_linked_name() {
        let mut c = Config::default();
        let s1 = yerd_core::Site::linked("api", "/a", yerd_core::PhpVersion::new(8, 3)).unwrap();
        let s2 = yerd_core::Site::linked("api", "/b", yerd_core::PhpVersion::new(8, 3)).unwrap();
        c.linked.push(s1);
        c.linked.push(s2);
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::DuplicateLinkedSite,
            }) => {}
            other => panic!("expected DuplicateLinkedSite, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_empty_parked_path() {
        let mut c = Config::default();
        c.parked.paths.insert(String::new());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::ParkedPathEmpty,
            }) => {}
            other => panic!("expected ParkedPathEmpty, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_unknown_service() {
        let mut c = Config::default();
        c.services
            .instances
            .insert("sqlserver".to_string(), ServiceInstance::default());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::UnknownService,
            }) => {}
            other => panic!("expected UnknownService, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_each_known_service() {
        for s in KNOWN_SERVICES {
            let mut c = Config::default();
            c.services
                .instances
                .insert((*s).to_string(), ServiceInstance::default());
            c.validate().unwrap_or_else(|e| panic!("rejected {s}: {e}"));
        }
    }

    #[test]
    fn validate_returns_first_failure_in_documented_order() {
        let mut c = Config::default();
        c.ports.http = 0;
        let s1 = yerd_core::Site::linked("api", "/a", yerd_core::PhpVersion::new(8, 3)).unwrap();
        let s2 = yerd_core::Site::linked("api", "/b", yerd_core::PhpVersion::new(8, 3)).unwrap();
        c.linked.push(s1);
        c.linked.push(s2);
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::HttpPortZero,
            }) => {}
            other => panic!("(a) expected HttpPortZero, got {other:?}"),
        }

        let mut c = Config::default();
        c.ports.http = 9000;
        c.ports.https = 9000;
        let s1 = yerd_core::Site::linked("api", "/a", yerd_core::PhpVersion::new(8, 3)).unwrap();
        let s2 = yerd_core::Site::linked("api", "/b", yerd_core::PhpVersion::new(8, 3)).unwrap();
        c.linked.push(s1);
        c.linked.push(s2);
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::HttpHttpsPortsEqual,
            }) => {}
            other => panic!("(b) expected HttpHttpsPortsEqual, got {other:?}"),
        }

        let mut c = Config::default();
        let s1 = yerd_core::Site::linked("api", "/a", yerd_core::PhpVersion::new(8, 3)).unwrap();
        let s2 = yerd_core::Site::linked("api", "/b", yerd_core::PhpVersion::new(8, 3)).unwrap();
        c.linked.push(s1);
        c.linked.push(s2);
        c.parked.paths.insert(String::new());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::DuplicateLinkedSite,
            }) => {}
            other => panic!("(c) expected DuplicateLinkedSite, got {other:?}"),
        }

        let mut c = Config::default();
        c.parked.paths.insert(String::new());
        c.overrides
            .insert(String::new(), crate::SiteOverride::default());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::ParkedPathEmpty,
            }) => {}
            other => panic!("(d) expected ParkedPathEmpty, got {other:?}"),
        }

        let mut c = Config::default();
        c.overrides
            .insert(String::new(), crate::SiteOverride::default());
        c.services
            .instances
            .insert("sqlserver".to_string(), ServiceInstance::default());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::OverridePathEmpty,
            }) => {}
            other => panic!("(f) expected OverridePathEmpty, got {other:?}"),
        }

        let mut c = Config::default();
        c.services
            .instances
            .insert("sqlserver".to_string(), ServiceInstance::default());
        c.php
            .settings
            .insert("memory_limit".to_string(), "bogus".to_string());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::UnknownService,
            }) => {}
            other => panic!("(e) expected UnknownService, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_unsupported_and_bad_php_setting() {
        let mut c = Config::default();
        c.php
            .settings
            .insert("allow_url_fopen".to_string(), "1".to_string());
        assert!(matches!(
            c.validate(),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::InvalidPhpSetting,
            })
        ));

        let mut c = Config::default();
        c.php
            .settings
            .insert("memory_limit".to_string(), "256M; evil".to_string());
        assert!(matches!(
            c.validate(),
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::InvalidPhpSetting,
            })
        ));
    }

    #[test]
    fn default_config_omits_dumps_table() {
        let s = Config::default().to_toml().unwrap();
        assert!(
            !s.contains("[dumps]"),
            "default config must omit the dumps table; got: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back.dumps, crate::DumpsSection::default());
    }

    #[test]
    fn dumps_section_round_trips_through_toml() {
        let mut c = Config::default();
        c.dumps.enabled = true;
        c.dumps.port = 2400;
        c.dumps.features.insert("queries".to_string(), false);
        let s = c.to_toml().unwrap();
        assert!(s.contains("[dumps]"), "expected [dumps] table; got: {s}");
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.dumps.port, 2400);
        assert!(back.dumps.enabled);
        assert_eq!(back.dumps.features.get("queries"), Some(&false));
    }

    #[test]
    fn v3_config_without_dumps_migrates_to_default_dumps() {
        let c = Config::from_toml("version = 3\n").unwrap();
        assert_eq!(c.dumps, crate::DumpsSection::default());
    }

    #[test]
    fn php_extensions_round_trip_and_default_name() {
        let s = "version = 10\n[php]\ndefault = \"8.3\"\n\
                 [[php.extensions.\"8.5\"]]\n\
                 path = \"/opt/php/pecl/scrypt.so\"\nzend = false\n";
        let c = Config::from_toml(s).unwrap();
        let v = c
            .php
            .extensions
            .get(&yerd_core::PhpVersion::new(8, 5))
            .unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "scrypt");
        assert_eq!(v[0].path, "/opt/php/pecl/scrypt.so");
        assert!(!v[0].zend);
    }

    #[test]
    fn validate_rejects_invalid_extension_path() {
        let mut c = Config::default();
        c.php.extensions.insert(
            yerd_core::PhpVersion::new(8, 5),
            vec![crate::ExtEntry {
                name: "scrypt".to_string(),
                path: "relative/scrypt.so".to_string(),
                zend: false,
            }],
        );
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::InvalidPhpExtension,
            }) => {}
            other => panic!("expected InvalidPhpExtension, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_duplicate_extension_name_within_version() {
        let mut c = Config::default();
        c.php.extensions.insert(
            yerd_core::PhpVersion::new(8, 5),
            vec![
                crate::ExtEntry {
                    name: "dup".to_string(),
                    path: "/a/one.so".to_string(),
                    zend: false,
                },
                crate::ExtEntry {
                    name: "dup".to_string(),
                    path: "/a/two.so".to_string(),
                    zend: false,
                },
            ],
        );
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::DuplicateExtensionName,
            }) => {}
            other => panic!("expected DuplicateExtensionName, got {other:?}"),
        }
    }

    #[test]
    fn php_settings_round_trip_through_toml() {
        let mut c = Config::default();
        c.php
            .settings
            .insert("memory_limit".to_string(), "512M".to_string());
        c.php
            .settings
            .insert("max_execution_time".to_string(), "300".to_string());
        let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
