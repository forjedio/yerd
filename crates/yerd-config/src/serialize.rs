//! TOML serialisation via crate-internal borrowed wire mirrors.
//!
//! Public schema types do not derive `Serialize` (see `schema.rs` rustdocs).
//! Serialisation routes through private mirror structs that hold borrowed
//! references into the public types, then [`toml::to_string_pretty`].

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{Config, ConfigError, CURRENT_VERSION};

#[derive(Serialize)]
struct WireSer<'a> {
    // MUST remain first — TOML emits scalars before sub-tables in a parent,
    // and within scalars `to_string_pretty` follows struct field order.
    // Pinned by tests/toml_byte_shape.rs::default_config_starts_with_version_line.
    version: u32,
    tld: &'a yerd_core::Tld,
    ports: PortsSer<'a>,
    php: PhpSectionSer<'a>,
    parked: ParkedSectionSer<'a>,
    linked: &'a [yerd_core::Site],
    services: ServicesSectionSer<'a>,
}

#[derive(Serialize)]
struct PortsSer<'a> {
    http: &'a u16,
    https: &'a u16,
}

#[derive(Serialize)]
struct PhpSectionSer<'a> {
    default: &'a yerd_core::PhpVersion,
}

#[derive(Serialize)]
struct ParkedSectionSer<'a> {
    paths: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct ServicesSectionSer<'a> {
    enabled: &'a BTreeSet<String>,
}

pub(crate) fn to_toml(c: &Config) -> Result<String, ConfigError> {
    let w = WireSer {
        version: CURRENT_VERSION,
        tld: &c.tld,
        ports: PortsSer {
            http: &c.ports.http,
            https: &c.ports.https,
        },
        php: PhpSectionSer {
            default: &c.php.default,
        },
        parked: ParkedSectionSer {
            paths: &c.parked.paths,
        },
        linked: &c.linked,
        services: ServicesSectionSer {
            enabled: &c.services.enabled,
        },
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
            s.starts_with("version = 1\n"),
            "expected `version = 1` first line; got: {s}"
        );
    }

    #[test]
    fn default_to_toml_parses_back_to_default() {
        let s = to_toml(&Config::default()).unwrap();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(back, Config::default());
    }
}
