//! TOML serialisation via crate-internal borrowed wire mirrors.
//!
//! Public schema types do not derive `Serialize` (see `schema.rs` rustdocs).
//! Serialisation routes through private mirror structs that hold borrowed
//! references into the public types, then [`toml::to_string_pretty`].

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{Config, ConfigError, CURRENT_VERSION};

#[derive(Serialize)]
struct WireSer<'a> {
    // MUST remain first - TOML emits scalars before sub-tables in a parent,
    // and within scalars `to_string_pretty` follows struct field order.
    // Pinned by tests/toml_byte_shape.rs::default_config_starts_with_version_line.
    version: u32,
    tld: &'a yerd_core::Tld,
    // Scalar - must stay above the sub-tables (TOML emits scalars before tables).
    dns_port: u16,
    // v6 scalar - also above the sub-tables. Always emitted (like `tld` /
    // `dns_port`) so the channel is visible/editable in the file.
    update_channel: &'a str,
    // v12 scalar - must stay in the scalar region above the sub-tables. Always
    // emitted so the toggle is visible/editable in the file.
    symlink_protection: bool,
    ports: PortsSer<'a>,
    php: PhpSectionSer<'a>,
    parked: ParkedSectionSer<'a>,
    linked: &'a [yerd_core::Site],
    // Array-of-tables (`[[overrides]]`), a sub-table region like `linked` - any
    // order among the tables is fine for `to_string_pretty`. Skipped when empty
    // so a default config emits no `[[overrides]]` (load-bearing for the
    // byte-shape goldens, which assume no extra tables).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    overrides: Vec<OverrideSer<'a>>,
    // v3: per-service tables (`[services.redis]`). Skipped when empty so a
    // default config emits no `[services]` region (byte-shape goldens assume no
    // extra tables). `BTreeMap` → deterministic lexicographic table order.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    services: BTreeMap<&'a str, ServiceInstanceSer<'a>>,
    // v4: built-in mail-capture server (`[mail]`). A trailing sub-table region,
    // so keeping it after `services` leaves the byte order of every existing
    // table unchanged. `None` (skipped) when the section equals its default, so
    // a default config emits no `[mail]` table (byte-shape goldens assume no
    // extra tables).
    #[serde(skip_serializing_if = "Option::is_none")]
    mail: Option<MailSectionSer>,
    // v5: optional `[dumps]` table - another trailing sub-table region. `None`
    // (skipped) when the section equals its default, so a default config emits
    // no `[dumps]` region, keeping the byte-shape goldens intact.
    #[serde(skip_serializing_if = "Option::is_none")]
    dumps: Option<DumpsSectionSer<'a>>,
    // v8: optional `[tunnel]` table - a trailing sub-table region. `None`
    // (skipped) when both maps are empty, so a default config emits no `[tunnel]`
    // region, keeping the byte-shape goldens intact.
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel: Option<TunnelSectionSer<'a>>,
    // v9: optional `[groups]` table - a trailing sub-table region. `None`
    // (skipped) when the section is empty, so a default config emits no
    // `[groups]` region, keeping the byte-shape goldens intact.
    #[serde(skip_serializing_if = "Option::is_none")]
    groups: Option<GroupsSectionSer<'a>>,
    // v11: optional `[domains]` table - a trailing sub-table region. `None`
    // (skipped) when the section is empty, so a default config emits no
    // `[domains]` region, keeping the byte-shape goldens intact.
    #[serde(skip_serializing_if = "Option::is_none")]
    domains: Option<DomainsSectionSer<'a>>,
    // v14: optional `[[proxies]]` array - a trailing sub-table region. Skipped
    // when empty so a default config emits no `[[proxies]]`, keeping the
    // byte-shape goldens intact.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    proxies: Vec<ProxySer<'a>>,
    // v14: optional `[proxy_rules]` table - a trailing sub-table region. `None`
    // (skipped) when both maps are empty, so a default config emits no
    // `[proxy_rules]` region.
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_rules: Option<ProxyRulesSectionSer<'a>>,
}

#[derive(Serialize)]
struct ProxySer<'a> {
    name: &'a str,
    target: String,
    // A default (off) `secure` still emits, since every `[[proxies]]` entry
    // already emits a table; keeping the field present makes the flag visible.
    secure: bool,
}

#[derive(Serialize)]
struct ProxyRulesSectionSer<'a> {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    linked: BTreeMap<&'a str, Vec<ProxyRuleSer<'a>>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    parked: BTreeMap<&'a str, Vec<ProxyRuleSer<'a>>>,
}

#[derive(Serialize)]
struct ProxyRuleSer<'a> {
    prefix: &'a str,
    target: String,
}

#[derive(Serialize)]
struct DomainsSectionSer<'a> {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    linked: BTreeMap<&'a str, DomainDeltaSer<'a>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    parked: BTreeMap<&'a str, DomainDeltaSer<'a>>,
}

#[derive(Serialize)]
struct DomainDeltaSer<'a> {
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    added: Vec<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    suppressed: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    primary: Option<&'a str>,
}

#[derive(Serialize)]
struct TunnelSectionSer<'a> {
    #[serde(skip_serializing_if = "map_is_empty")]
    named: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "map_is_empty")]
    sites: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct GroupsSectionSer<'a> {
    // Scalar array - must stay above the `members` sub-table (TOML emits scalars
    // before sub-tables within `[groups]`).
    order: &'a [String],
    // Skipped when empty so a membership-free `[groups]` emits no
    // `[groups.members]` table.
    #[serde(skip_serializing_if = "map_is_empty")]
    members: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct DumpsSectionSer<'a> {
    // Scalars first (TOML emits scalars before sub-tables within `[dumps]`).
    enabled: bool,
    port: u16,
    persist: bool,
    // Skipped when empty so a feature-override-free `[dumps]` emits no
    // `[dumps.features]` table.
    #[serde(skip_serializing_if = "bool_map_is_empty")]
    features: &'a BTreeMap<String, bool>,
}

/// `skip_serializing_if` predicate for the borrowed `features` field.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn bool_map_is_empty(m: &&BTreeMap<String, bool>) -> bool {
    m.is_empty()
}

#[derive(Serialize)]
struct PortsSer<'a> {
    http: &'a u16,
    https: &'a u16,
    fallback_http: &'a u16,
    fallback_https: &'a u16,
}

#[derive(Serialize)]
struct PhpSectionSer<'a> {
    // Scalar - must stay above the `settings`/`extensions` sub-tables (TOML emits
    // scalars before sub-tables within `[php]`).
    default: &'a yerd_core::PhpVersion,
    // Skipped when empty so a settings-free config has no `[php.settings]`
    // table. `skip_serializing_if` receives `&&BTreeMap`, hence `map_is_empty`.
    #[serde(skip_serializing_if = "map_is_empty")]
    settings: &'a BTreeMap<String, String>,
    // Array-of-tables keyed by version string (`[[php.extensions."8.5"]]`).
    // Skipped when empty so a default config emits no `[php.extensions]` region
    // (byte-shape goldens assume no extra tables). A trailing sub-table region,
    // so it stays after `settings`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    extensions: BTreeMap<String, Vec<ExtEntrySer<'a>>>,
}

#[derive(Serialize)]
struct ExtEntrySer<'a> {
    name: &'a str,
    path: &'a str,
    zend: bool,
}

/// `skip_serializing_if` predicate for the borrowed `settings` field. serde
/// dictates the `&&BTreeMap` signature (the field is already `&BTreeMap`).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn map_is_empty(m: &&BTreeMap<String, String>) -> bool {
    m.is_empty()
}

#[derive(Serialize)]
struct ParkedSectionSer<'a> {
    paths: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct ServiceInstanceSer<'a> {
    // Per-field skip so an unpinned instance emits only `enabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    enabled: bool,
}

/// `[mail]` table. Owned (not borrowed) - the fields are `Copy` scalars, so
/// there's nothing to borrow. Emitted only when the section is non-default.
#[derive(Serialize)]
struct MailSectionSer {
    enabled: bool,
    port: u16,
}

#[derive(Serialize)]
struct OverrideSer<'a> {
    path: &'a str,
    // Per-field skip so an override that pins only one value emits only that
    // key (no `php = ""` / `secure = false` noise).
    #[serde(skip_serializing_if = "Option::is_none")]
    php: Option<&'a yerd_core::PhpVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secure: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wp_auto_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wp_auto_login_user: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    front_controller: Option<bool>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn to_toml(c: &Config) -> Result<String, ConfigError> {
    let w = WireSer {
        version: CURRENT_VERSION,
        tld: &c.tld,
        dns_port: c.dns_port,
        update_channel: &c.update_channel,
        symlink_protection: c.symlink_protection,
        ports: PortsSer {
            http: &c.ports.http,
            https: &c.ports.https,
            fallback_http: &c.ports.fallback_http,
            fallback_https: &c.ports.fallback_https,
        },
        php: PhpSectionSer {
            default: &c.php.default,
            settings: &c.php.settings,
            extensions: c
                .php
                .extensions
                .iter()
                .map(|(v, entries)| {
                    (
                        v.to_string(),
                        entries
                            .iter()
                            .map(|e| ExtEntrySer {
                                name: &e.name,
                                path: &e.path,
                                zend: e.zend,
                            })
                            .collect(),
                    )
                })
                .collect(),
        },
        parked: ParkedSectionSer {
            paths: &c.parked.paths,
        },
        linked: &c.linked,
        overrides: c
            .overrides
            .iter()
            .map(|(path, ov)| OverrideSer {
                path,
                php: ov.php.as_ref(),
                secure: ov.secure,
                web_root: ov.web_root.as_deref(),
                wp_auto_login: ov.wp_auto_login,
                wp_auto_login_user: ov.wp_auto_login_user.as_deref(),
                front_controller: ov.front_controller,
            })
            .collect(),
        services: c
            .services
            .instances
            .iter()
            .map(|(name, inst)| {
                (
                    name.as_str(),
                    ServiceInstanceSer {
                        version: inst.version.as_deref(),
                        port: inst.port,
                        enabled: inst.enabled,
                    },
                )
            })
            .collect(),
        mail: if c.mail == crate::MailSection::default() {
            None
        } else {
            Some(MailSectionSer {
                enabled: c.mail.enabled,
                port: c.mail.port,
            })
        },
        dumps: if c.dumps == crate::schema::DumpsSection::default() {
            None
        } else {
            Some(DumpsSectionSer {
                enabled: c.dumps.enabled,
                port: c.dumps.port,
                persist: c.dumps.persist,
                features: &c.dumps.features,
            })
        },
        tunnel: if c.tunnel == crate::schema::TunnelSection::default() {
            None
        } else {
            Some(TunnelSectionSer {
                named: &c.tunnel.named,
                sites: &c.tunnel.sites,
            })
        },
        groups: if c.groups.is_empty() {
            None
        } else {
            Some(GroupsSectionSer {
                order: &c.groups.order,
                members: &c.groups.members,
            })
        },
        domains: if c
            .domains
            .linked
            .values()
            .chain(c.domains.parked.values())
            .all(crate::schema::DomainDelta::is_empty)
        {
            None
        } else {
            Some(DomainsSectionSer {
                linked: domain_delta_map(&c.domains.linked),
                parked: domain_delta_map(&c.domains.parked),
            })
        },
        proxies: c
            .proxies
            .iter()
            .map(|p| ProxySer {
                name: p.name(),
                target: p.target().to_string(),
                secure: p.secure(),
            })
            .collect(),
        proxy_rules: {
            let linked = proxy_rule_map(&c.proxy_rules.linked);
            let parked = proxy_rule_map(&c.proxy_rules.parked);
            if linked.is_empty() && parked.is_empty() {
                None
            } else {
                Some(ProxyRulesSectionSer { linked, parked })
            }
        },
    };
    toml::to_string_pretty(&w).map_err(Into::into)
}

/// Build a borrowed `[proxy_rules.*]` map, pruning any site whose rule list is
/// empty (an empty list is equivalent to no entry and must not emit `key = []`,
/// so removing a site's last rule round-trips to a byte-identical config).
fn proxy_rule_map(
    src: &BTreeMap<String, Vec<yerd_core::ProxyRule>>,
) -> BTreeMap<&str, Vec<ProxyRuleSer<'_>>> {
    src.iter()
        .filter(|(_, rules)| !rules.is_empty())
        .map(|(k, rules)| {
            (
                k.as_str(),
                rules
                    .iter()
                    .map(|r| ProxyRuleSer {
                        prefix: r.prefix(),
                        target: r.target().to_string(),
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Build a borrowed `[domains.*]` delta map, pruning any all-empty delta (an
/// all-empty entry is equivalent to no customisation and must not emit a table).
fn domain_delta_map(
    src: &BTreeMap<String, crate::schema::DomainDelta>,
) -> BTreeMap<&str, DomainDeltaSer<'_>> {
    src.iter()
        .filter(|(_, d)| !d.is_empty())
        .map(|(k, d)| {
            (
                k.as_str(),
                DomainDeltaSer {
                    added: d.added.iter().map(yerd_core::Domain::as_str).collect(),
                    suppressed: d.suppressed.iter().map(yerd_core::Domain::as_str).collect(),
                    primary: d.primary.as_ref().map(yerd_core::Domain::as_str),
                },
            )
        })
        .collect()
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
    fn default_to_toml_starts_with_version_line() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            s.starts_with("version = 14\n"),
            "expected `version = 14` first line; got: {s}"
        );
    }

    #[test]
    fn default_config_emits_no_mail_table() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            !s.contains("[mail]"),
            "default config must omit the mail table; got: {s}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn non_default_mail_section_round_trips() {
        let mut c = Config::default();
        c.mail = crate::MailSection {
            enabled: true,
            port: 3030,
        };
        let s = to_toml(&c).unwrap();
        assert!(s.contains("[mail]"), "enabled mail must emit a table: {s}");
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn default_to_toml_parses_back_to_default() {
        let s = to_toml(&Config::default()).unwrap();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, Config::default());
    }

    #[test]
    fn default_config_emits_no_tunnel_table() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            !s.contains("[tunnel]"),
            "default config must omit the tunnel table; got: {s}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn non_default_tunnel_section_round_trips() {
        let mut c = Config::default();
        c.tunnel = crate::TunnelSection {
            named: BTreeMap::from([("mysite".to_owned(), "uuid-123".to_owned())]),
            sites: BTreeMap::from([("app".to_owned(), "app.example.com".to_owned())]),
        };
        let s = to_toml(&c).unwrap();
        assert!(s.contains("[tunnel.named]"), "must emit named table: {s}");
        assert!(s.contains("[tunnel.sites]"), "must emit sites table: {s}");
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn default_config_emits_no_groups_table() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            !s.contains("[groups]"),
            "default config must omit the groups table; got: {s}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn non_default_groups_section_round_trips() {
        let mut c = Config::default();
        c.groups = crate::GroupsSection {
            order: vec!["Blog".to_owned(), "Shop".to_owned()],
            members: BTreeMap::from([("api".to_owned(), "Blog".to_owned())]),
        };
        let s = to_toml(&c).unwrap();
        assert!(s.contains("[groups]"), "must emit groups table: {s}");
        assert!(
            s.contains("[groups.members]"),
            "must emit members table: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn default_config_emits_no_domains_table() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            !s.contains("[domains"),
            "default config must omit the domains table; got: {s}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn populated_domains_section_round_trips() {
        use yerd_core::Domain;
        let mut c = Config::default();
        c.domains.linked.insert(
            "blog".to_owned(),
            crate::DomainDelta {
                added: vec![
                    Domain::parse_subpart("corp").unwrap(),
                    Domain::parse_subpart("*.blog").unwrap(),
                ],
                suppressed: vec![Domain::parse_subpart("blog").unwrap()],
                primary: Some(Domain::parse_subpart("corp").unwrap()),
            },
        );
        c.domains.parked.insert(
            "/srv/shop".to_owned(),
            crate::DomainDelta {
                added: vec![Domain::parse_subpart("shop-alias").unwrap()],
                suppressed: vec![],
                primary: None,
            },
        );
        let s = to_toml(&c).unwrap();
        assert!(
            s.contains("[domains.linked.blog]"),
            "must emit linked delta: {s}"
        );
        assert!(
            s.contains("[domains.parked.\"/srv/shop\"]"),
            "must emit parked delta keyed by docroot: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn all_empty_domain_delta_omits_table_and_is_not_round_tripped() {
        // An all-empty delta is equivalent to no customisation: the serialiser
        // prunes it, so the emitted config is byte-identical to the default.
        let baseline = to_toml(&Config::default()).unwrap();
        let mut c = Config::default();
        c.domains
            .linked
            .insert("blog".to_owned(), crate::DomainDelta::default());
        assert_eq!(to_toml(&c).unwrap(), baseline);
    }

    #[test]
    fn default_config_emits_no_php_extensions_table() {
        let s = to_toml(&Config::default()).unwrap();
        assert!(
            !s.contains("[php.extensions"),
            "default config must omit the php.extensions table; got: {s}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn php_extensions_emit_array_of_tables_and_round_trip() {
        use yerd_core::PhpVersion;
        let mut c = Config::default();
        c.php.extensions.insert(
            PhpVersion::new(8, 5),
            vec![
                crate::ExtEntry {
                    name: "scrypt".to_owned(),
                    path: "/opt/homebrew/lib/php/pecl/20250925/scrypt.so".to_owned(),
                    zend: false,
                },
                crate::ExtEntry {
                    name: "xdebug".to_owned(),
                    path: "/opt/homebrew/lib/php/pecl/20250925/xdebug.so".to_owned(),
                    zend: true,
                },
            ],
        );
        let s = to_toml(&c).unwrap();
        assert!(
            s.contains("[[php.extensions.\"8.5\"]]"),
            "expected array-of-tables under the version key; got: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn removing_last_extension_returns_to_byte_identical_default() {
        use yerd_core::PhpVersion;
        let baseline = to_toml(&Config::default()).unwrap();
        let mut c = Config::default();
        c.php.extensions.insert(
            PhpVersion::new(8, 5),
            vec![crate::ExtEntry {
                name: "scrypt".to_owned(),
                path: "/a/scrypt.so".to_owned(),
                zend: false,
            }],
        );
        c.php.extensions.clear();
        assert_eq!(to_toml(&c).unwrap(), baseline);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn empty_members_groups_omits_members_table() {
        let mut c = Config::default();
        c.groups = crate::GroupsSection {
            order: vec!["Blog".to_owned()],
            members: BTreeMap::new(),
        };
        let s = to_toml(&c).unwrap();
        assert!(s.contains("[groups]"), "must emit groups table: {s}");
        assert!(
            !s.contains("[groups.members]"),
            "membership-free groups must omit members table; got: {s}"
        );
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, c);
    }
}
