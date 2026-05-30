//! TOML round-trip integration tests for `yerd-config`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use yerd_config::{Config, PhpSection, Ports, SiteOverride};
use yerd_core::{PhpVersion, Site, Tld};

const POPULATED: &str = r#"
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

[[overrides]]
path = "docroot-a/blog"
php = "8.4"
secure = true

[services]
enabled = ["mysql", "redis"]
"#;

fn populated_expected() -> Config {
    let mut c = Config::default();
    c.tld = Tld::new("test").unwrap();
    c.ports = Ports {
        http: 8080,
        https: 8443,
    };
    c.php = PhpSection {
        default: PhpVersion::new(8, 2),
        settings: std::collections::BTreeMap::new(),
    };
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
        },
    );
    c.services.enabled.insert("mysql".to_string());
    c.services.enabled.insert("redis".to_string());
    c
}

#[test]
fn default_round_trip() {
    let s = Config::default().to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back, Config::default());
}

#[test]
fn populated_round_trip() {
    let parsed = Config::from_toml(POPULATED).unwrap();
    assert_eq!(parsed, populated_expected());
    let s = parsed.to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back, parsed);
}

#[test]
fn populated_round_trip_passes_validate() {
    let parsed = Config::from_toml(POPULATED).unwrap();
    parsed.validate().unwrap();
}

#[test]
fn default_to_toml_then_from_toml_pins_php_version_default() {
    let s = Config::default().to_toml().unwrap();
    let back = Config::from_toml(&s).unwrap();
    assert_eq!(back.php.default, PhpVersion::new(8, 3));
}
