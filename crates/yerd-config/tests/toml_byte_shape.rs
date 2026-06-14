//! Structural goldens and spot-check substring assertions on the TOML the
//! serialiser emits. Tests are chosen so they survive `to_string_pretty`'s
//! line-break and table-ordering choices.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;

use yerd_config::{Config, ServiceInstance, SiteOverride};
use yerd_core::{PhpVersion, Site, Tld};

fn populated() -> Config {
    let mut c = Config::default();
    c.tld = Tld::new("test").unwrap();
    c.ports.http = 8080;
    c.ports.https = 8443;
    c.php.default = PhpVersion::new(8, 2);
    c.parked.paths.insert("docroot-a".to_string());
    c.parked.paths.insert("docroot-b".to_string());
    let mut site = Site::linked("api", "docroot", PhpVersion::new(8, 3)).unwrap();
    site.set_secure(true);
    c.linked.push(site);
    c.overrides.insert(
        "docroot-a/blog".to_string(),
        SiteOverride {
            php: Some(PhpVersion::new(8, 4)),
            secure: Some(true),
            web_root: None,
        },
    );
    c.services.instances.insert(
        "mysql".to_string(),
        ServiceInstance {
            version: None,
            port: None,
            enabled: true,
        },
    );
    c.services.instances.insert(
        "redis".to_string(),
        ServiceInstance {
            version: Some("8".to_string()),
            port: Some(6380),
            enabled: true,
        },
    );
    c
}

#[test]
fn default_config_starts_with_version_line() {
    let s = Config::default().to_toml().unwrap();
    assert!(
        s.starts_with("version = 4\n"),
        "expected first line `version = 4`; got: {s}"
    );
}

#[test]
fn default_config_emits_dns_port_scalar_before_tables() {
    let s = Config::default().to_toml().unwrap();
    // Default is the fixed loopback DNS port, emitted as a top-level scalar
    // (before any `[section]` table).
    assert!(
        s.contains("dns_port = 1053\n"),
        "expected `dns_port = 1053` scalar; got: {s}"
    );
    let dns_at = s.find("dns_port = ").expect("dns_port present");
    let first_table = s.find("\n[").expect("at least one table");
    assert!(dns_at < first_table, "dns_port must precede tables in: {s}");
    // And it round-trips.
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back.dns_port, 1053);
}

#[test]
fn dns_port_zero_round_trips() {
    let mut c = Config::default();
    c.dns_port = 0;
    let back = Config::from_toml(&c.to_toml().unwrap()).unwrap();
    assert_eq!(back.dns_port, 0);
}

#[test]
fn default_config_contains_each_section_header() {
    let s = Config::default().to_toml().unwrap();
    // `[services]` is omitted when empty (v3: per-service tables, skipped like
    // `[[overrides]]` / `[php.settings]`).
    for header in ["\n[ports]\n", "\n[php]\n", "\n[parked]\n"] {
        assert!(
            s.contains(header),
            "missing section header `{header}` in: {s}"
        );
    }
    assert!(
        !s.contains("[services"),
        "default config must omit the services table; got: {s}"
    );
}

#[test]
fn populated_config_uses_double_bracket_linked_form() {
    let s = populated().to_toml().unwrap();
    assert!(
        s.contains("\n[[linked]]\n"),
        "missing `[[linked]]` header in: {s}"
    );
}

#[test]
fn populated_config_uses_double_bracket_override_form() {
    let s = populated().to_toml().unwrap();
    assert!(
        s.contains("\n[[overrides]]\n"),
        "missing `[[overrides]]` header in: {s}"
    );
    // Round-trips back to the same overrides map.
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back.overrides, populated().overrides);
}

#[test]
fn empty_overrides_emit_no_table() {
    // A config with no overrides must not carry any `[[overrides]]` table.
    let s = Config::default().to_toml().unwrap();
    assert!(
        !s.contains("[[overrides]]"),
        "empty overrides must omit the table; got: {s}"
    );
}

#[test]
fn default_config_emits_no_mail_table() {
    // The default (disabled) mail section must not carry a `[mail]` table.
    let s = Config::default().to_toml().unwrap();
    assert!(
        !s.contains("[mail]"),
        "default mail section must omit the table; got: {s}"
    );
}

#[test]
fn override_with_only_php_omits_secure_key() {
    let mut c = Config::default();
    c.overrides.insert(
        "/srv/blog".to_string(),
        SiteOverride {
            php: Some(PhpVersion::new(8, 4)),
            secure: None,
            web_root: None,
        },
    );
    let s = c.to_toml().unwrap();
    let v: toml::Value = toml::from_str(&s).unwrap();
    let table = &v.get("overrides").expect("override array")[0];
    assert!(table.get("php").is_some(), "php should be present: {s}");
    assert!(
        table.get("secure").is_none(),
        "secure should be omitted when None: {s}"
    );
}

#[test]
fn parked_paths_emitted_in_lex_order() {
    // Insert in reverse alphabetical order; BTreeSet sorts to "a" before "b".
    let mut c = Config::default();
    c.parked.paths.insert("b".to_string());
    c.parked.paths.insert("a".to_string());
    let s = c.to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    let got: Vec<&String> = back.parked.paths.iter().collect();
    assert_eq!(got, vec![&"a".to_string(), &"b".to_string()]);
}

#[test]
fn services_tables_emitted_in_lex_order_and_round_trip() {
    let mut c = Config::default();
    c.services
        .instances
        .insert("redis".to_string(), ServiceInstance::default());
    c.services
        .instances
        .insert("mysql".to_string(), ServiceInstance::default());
    let s = c.to_toml().unwrap();
    // BTreeMap iteration → `[services.mysql]` is emitted before `[services.redis]`.
    let mysql_at = s.find("[services.mysql]").expect("mysql table present");
    let redis_at = s.find("[services.redis]").expect("redis table present");
    assert!(
        mysql_at < redis_at,
        "services tables must be lex-ordered: {s}"
    );
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back, c);
}

#[test]
fn service_instance_wire_shape_is_per_service_table() {
    // v3: each enabled service is a `[services.<id>]` table carrying `enabled`
    // (+ optional version/port) — NOT the old `enabled = [...]` array.
    let mut c = Config::default();
    c.services.instances.insert(
        "redis".to_string(),
        ServiceInstance {
            version: Some("8".to_string()),
            port: Some(6380),
            enabled: true,
        },
    );
    let s = c.to_toml().unwrap();
    let v: toml::Value = toml::from_str(&s).unwrap();
    let redis = v
        .get("services")
        .and_then(|x| x.get("redis"))
        .and_then(|x| x.as_table())
        .unwrap_or_else(|| panic!("missing [services.redis] table in: {s}"));
    assert_eq!(redis.get("enabled"), Some(&toml::Value::Boolean(true)));
    assert_eq!(redis.get("version"), Some(&toml::Value::String("8".into())));
    assert_eq!(redis.get("port"), Some(&toml::Value::Integer(6380)));
    // An unset value must be omitted (no `version = ""` noise) — inspect the
    // service's own table, not the whole doc (which carries a top-level
    // `version = 4` line).
    let mut c2 = Config::default();
    c2.services
        .instances
        .insert("mysql".to_string(), ServiceInstance::default());
    let s2 = c2.to_toml().unwrap();
    let v2: toml::Value = toml::from_str(&s2).unwrap();
    let mysql = v2
        .get("services")
        .and_then(|x| x.get("mysql"))
        .and_then(|x| x.as_table())
        .expect("expected [services.mysql] table");
    assert!(
        mysql.get("version").is_none(),
        "unset version must be omitted: {s2}"
    );
    assert!(
        mysql.get("port").is_none(),
        "unset port must be omitted: {s2}"
    );
    assert_eq!(mysql.get("enabled"), Some(&toml::Value::Boolean(true)));
}

#[test]
fn structural_round_trip_matches_input() {
    let parsed = Config::from_toml(
        r#"
version = 1
tld = "test"

[ports]
http = 8080
https = 8443

[php]
default = "8.2"

[parked]
paths = ["docroot-a", "docroot-b"]

[[linked]]
name = "api"
document_root = "docroot"
php = "8.3"
secure = true
kind = "linked"

[services]
enabled = ["mysql", "redis"]
"#,
    )
    .unwrap();
    let s = parsed.to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back, parsed);
}

#[test]
fn empty_php_settings_emit_no_subtable() {
    // A settings-free config must not carry a `[php.settings]` table.
    let s = Config::default().to_toml().unwrap();
    assert!(
        !s.contains("[php.settings]"),
        "empty settings must omit the table; got: {s}"
    );
}

#[test]
fn populated_php_settings_emit_subtable_after_default_and_round_trip() {
    let mut c = Config::default();
    c.php
        .settings
        .insert("memory_limit".to_string(), "512M".to_string());
    c.php
        .settings
        .insert("display_errors".to_string(), "On".to_string());
    let s = c.to_toml().unwrap();

    // The `default` scalar must precede the `[php.settings]` sub-table.
    let php_at = s.find("\n[php]\n").expect("[php] table present");
    let settings_at = s.find("[php.settings]").expect("[php.settings] present");
    assert!(
        php_at < settings_at,
        "default scalar must precede [php.settings]; got: {s}"
    );

    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back, c);
    assert_eq!(
        back.php.settings.get("memory_limit").map(String::as_str),
        Some("512M")
    );
}

#[test]
fn empty_parked_emits_empty_array_and_services_omitted() {
    let c = Config::default();
    let s = c.to_toml().unwrap();
    let v: toml::Value = toml::from_str(&s).unwrap();
    // `parked.paths` still serialises as an explicit empty array.
    let paths = v
        .get("parked")
        .and_then(|x| x.get("paths"))
        .and_then(|x| x.as_array())
        .expect("expected parked.paths array");
    assert!(paths.is_empty());
    // v3: an empty services map is omitted entirely (no `[services]` table).
    assert!(
        v.get("services").is_none(),
        "empty services must be omitted; got: {s}"
    );

    // Belt and braces: `BTreeSet::new()` here matches parked's storage.
    let _: BTreeSet<String> = BTreeSet::new();
}
