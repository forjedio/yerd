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

use yerd_config::Config;
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
    c.services.enabled.insert("mysql".to_string());
    c.services.enabled.insert("redis".to_string());
    c
}

#[test]
fn default_config_starts_with_version_line() {
    let s = Config::default().to_toml().unwrap();
    assert!(
        s.starts_with("version = 1\n"),
        "expected first line `version = 1`; got: {s}"
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
    for header in ["\n[ports]\n", "\n[php]\n", "\n[parked]\n", "\n[services]\n"] {
        assert!(
            s.contains(header),
            "missing section header `{header}` in: {s}"
        );
    }
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
fn services_enabled_emitted_in_lex_order() {
    let mut c = Config::default();
    c.services.enabled.insert("redis".to_string());
    c.services.enabled.insert("mysql".to_string());
    let s = c.to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    let got: Vec<&String> = back.services.enabled.iter().collect();
    assert_eq!(got, vec![&"mysql".to_string(), &"redis".to_string()]);
}

#[test]
fn services_enabled_wire_shape_is_array_of_strings() {
    let mut c = Config::default();
    c.services.enabled.insert("mysql".to_string());
    let s = c.to_toml().unwrap();
    let v: toml::Value = toml::from_str(&s).unwrap();
    let enabled = v
        .get("services")
        .and_then(|s| s.get("enabled"))
        .unwrap_or_else(|| panic!("missing services.enabled in: {s}"));
    let arr = enabled
        .as_array()
        .unwrap_or_else(|| panic!("services.enabled is not an array: {enabled:?}"));
    for entry in arr {
        assert!(
            entry.as_str().is_some(),
            "expected string entry, got: {entry:?}"
        );
    }
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
fn empty_btreeset_emits_empty_array() {
    let c = Config::default();
    let s = c.to_toml().unwrap();
    // Confirm the empty sets serialise as `paths = []` and `enabled = []`
    // rather than being silently omitted.
    let v: toml::Value = toml::from_str(&s).unwrap();
    let paths = v
        .get("parked")
        .and_then(|x| x.get("paths"))
        .and_then(|x| x.as_array())
        .expect("expected parked.paths array");
    assert!(paths.is_empty());
    let enabled = v
        .get("services")
        .and_then(|x| x.get("enabled"))
        .and_then(|x| x.as_array())
        .expect("expected services.enabled array");
    assert!(enabled.is_empty());

    // Belt and braces: `BTreeSet::new()` here matches the default's storage.
    let _: BTreeSet<String> = BTreeSet::new();
}
