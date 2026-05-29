//! TOML deserialisation, wire mirrors, and cross-field validation.
//!
//! The pipeline uses **raw-typed** wire mirrors and a `TryFrom<Wire>` for
//! [`Config`] conversion. Raw types let `yerd-core` validation failures
//! surface as typed [`ConfigError::Core`] rather than being folded into
//! [`ConfigError::Parse`] via `serde::de::Error::custom`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;

use crate::error::ValidateErrorReason;
use crate::schema::{Config, ParkedSection, PhpSection, Ports, ServicesSection};
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
    #[serde(default)]
    ports: PortsWire,
    #[serde(default)]
    php: PhpSectionWire,
    #[serde(default)]
    parked: ParkedSectionWire,
    #[serde(default)]
    linked: Vec<SiteWire>,
    #[serde(default)]
    services: ServicesSectionWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortsWire {
    http: u16,
    https: u16,
}

impl Default for PortsWire {
    fn default() -> Self {
        let p = Ports::default();
        Self {
            http: p.http,
            https: p.https,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhpSectionWire {
    default: String,
}

impl Default for PhpSectionWire {
    fn default() -> Self {
        Self {
            default: PhpSection::default().default.to_string(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ParkedSectionWire {
    #[serde(default)]
    paths: BTreeSet<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ServicesSectionWire {
    #[serde(default)]
    enabled: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteWire {
    name: String,
    document_root: PathBuf,
    php: String,
    secure: bool,
    kind: yerd_core::SiteKind,
}

fn default_tld_str() -> String {
    yerd_core::Tld::default().as_str().to_owned()
}

fn default_dns_port() -> u16 {
    crate::schema::DEFAULT_DNS_PORT
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

    fn try_from(w: Wire) -> Result<Self, ConfigError> {
        // Post-migration sanity check: wire.version must equal CURRENT_VERSION.
        // A STEPS misconfiguration that fails to bump it surfaces here.
        if w.version != crate::CURRENT_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: w.version,
                current: crate::CURRENT_VERSION,
            });
        }
        let tld = yerd_core::Tld::new(&w.tld)?;
        let php = PhpSection {
            default: yerd_core::PhpVersion::from_str(&w.php.default)?,
        };
        let ports = Ports {
            http: w.ports.http,
            https: w.ports.https,
        };
        let parked = ParkedSection {
            paths: w.parked.paths,
        };
        let services = ServicesSection {
            enabled: w.services.enabled,
        };
        let mut linked = Vec::with_capacity(w.linked.len());
        for sw in w.linked {
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
            linked.push(s);
        }
        Ok(Config {
            version: crate::CURRENT_VERSION,
            tld,
            dns_port: w.dns_port,
            ports,
            php,
            parked,
            linked,
            services,
        })
    }
}

pub(crate) fn validate(c: &Config) -> Result<(), ConfigError> {
    // Order is fixed for test predictability; pinned by
    // validate_returns_first_failure_in_documented_order.
    if c.ports.http == 0 {
        return Err(ve(ValidateErrorReason::HttpPortZero));
    }
    if c.ports.https == 0 {
        return Err(ve(ValidateErrorReason::HttpsPortZero));
    }
    if c.ports.http == c.ports.https {
        return Err(ve(ValidateErrorReason::HttpHttpsPortsEqual));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for s in &c.linked {
        if !seen.insert(s.name()) {
            return Err(ve(ValidateErrorReason::DuplicateLinkedSite));
        }
    }
    for p in &c.parked.paths {
        if p.is_empty() {
            return Err(ve(ValidateErrorReason::ParkedPathEmpty));
        }
    }
    for s in &c.services.enabled {
        if !KNOWN_SERVICES.iter().any(|k| *k == s.as_str()) {
            return Err(ve(ValidateErrorReason::UnknownService));
        }
    }
    Ok(())
}

fn ve(reason: ValidateErrorReason) -> ConfigError {
    ConfigError::Validate { reason }
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
                current: 1,
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
        assert!(c.services.enabled.is_empty());
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
        c.services.enabled.insert("sqlserver".to_string());
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
            c.services.enabled.insert((*s).to_string());
            c.validate().unwrap_or_else(|e| panic!("rejected {s}: {e}"));
        }
    }

    #[test]
    fn validate_returns_first_failure_in_documented_order() {
        // (a) http=0 + duplicate-linked → HttpPortZero
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

        // (b) http=https + duplicate-linked → HttpHttpsPortsEqual
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

        // (c) duplicate-linked + empty-parked → DuplicateLinkedSite
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

        // (d) empty-parked + unknown-service → ParkedPathEmpty
        let mut c = Config::default();
        c.parked.paths.insert(String::new());
        c.services.enabled.insert("sqlserver".to_string());
        match c.validate() {
            Err(ConfigError::Validate {
                reason: ValidateErrorReason::ParkedPathEmpty,
            }) => {}
            other => panic!("(d) expected ParkedPathEmpty, got {other:?}"),
        }
    }
}
