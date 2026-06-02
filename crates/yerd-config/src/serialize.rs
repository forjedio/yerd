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
    // MUST remain first — TOML emits scalars before sub-tables in a parent,
    // and within scalars `to_string_pretty` follows struct field order.
    // Pinned by tests/toml_byte_shape.rs::default_config_starts_with_version_line.
    version: u32,
    tld: &'a yerd_core::Tld,
    // Scalar — must stay above the sub-tables (TOML emits scalars before tables).
    dns_port: u16,
    ports: PortsSer<'a>,
    php: PhpSectionSer<'a>,
    parked: ParkedSectionSer<'a>,
    linked: &'a [yerd_core::Site],
    // Array-of-tables (`[[overrides]]`), a sub-table region like `linked` — any
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
}

#[derive(Serialize)]
struct PortsSer<'a> {
    http: &'a u16,
    https: &'a u16,
}

#[derive(Serialize)]
struct PhpSectionSer<'a> {
    // Scalar — must stay above the `settings` sub-table (TOML emits scalars
    // before sub-tables within `[php]`).
    default: &'a yerd_core::PhpVersion,
    // Skipped when empty so a settings-free config has no `[php.settings]`
    // table. `skip_serializing_if` receives `&&BTreeMap`, hence `map_is_empty`.
    #[serde(skip_serializing_if = "map_is_empty")]
    settings: &'a BTreeMap<String, String>,
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
}

pub(crate) fn to_toml(c: &Config) -> Result<String, ConfigError> {
    let w = WireSer {
        version: CURRENT_VERSION,
        tld: &c.tld,
        dns_port: c.dns_port,
        ports: PortsSer {
            http: &c.ports.http,
            https: &c.ports.https,
        },
        php: PhpSectionSer {
            default: &c.php.default,
            settings: &c.php.settings,
        },
        parked: ParkedSectionSer {
            paths: &c.parked.paths,
        },
        linked: &c.linked,
        // BTreeMap iteration is sorted → deterministic `[[overrides]]` order.
        overrides: c
            .overrides
            .iter()
            .map(|(path, ov)| OverrideSer {
                path,
                php: ov.php.as_ref(),
                secure: ov.secure,
                web_root: ov.web_root.as_deref(),
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
    };
    toml::to_string_pretty(&w).map_err(Into::into)
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
            s.starts_with("version = 3\n"),
            "expected `version = 3` first line; got: {s}"
        );
    }

    #[test]
    fn default_to_toml_parses_back_to_default() {
        let s = to_toml(&Config::default()).unwrap();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, Config::default());
    }
}
