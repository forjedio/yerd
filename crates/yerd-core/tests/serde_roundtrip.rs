//! Integration tests proving that `yerd-core` public types compose into the
//! shapes downstream crates need.
//!
//! - `ConfigShape` mirrors what `yerd-config` will load from TOML.
//! - `IpcSetPhp` mirrors a `yerd-ipc` request payload.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use yerd_core::{PhpVersion, Site, Tld};

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ConfigShape {
    php: PhpVersion,
    tld: Tld,
    sites: Vec<Site>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct IpcSetPhp {
    name: String,
    version: PhpVersion,
}

#[test]
fn toml_round_trip_config_shape() {
    let original = ConfigShape {
        php: PhpVersion::new(8, 3),
        tld: Tld::new("test").unwrap(),
        sites: vec![
            Site::parked("alpha", PathBuf::from("/srv/alpha"), PhpVersion::new(8, 3)).unwrap(),
            Site::linked("beta", PathBuf::from("/srv/beta"), PhpVersion::new(7, 4)).unwrap(),
        ],
    };

    let s = toml::to_string(&original).unwrap();
    // Sanity-check the human-readable shape.
    assert!(s.contains("php = \"8.3\""), "missing top-level php: {s}");
    assert!(s.contains("tld = \"test\""), "missing top-level tld: {s}");

    let back: ConfigShape = toml::from_str(&s).unwrap();
    assert_eq!(back, original);
}

#[test]
fn json_round_trip_ipc_set_php() {
    let original = IpcSetPhp {
        name: "alpha".into(),
        version: PhpVersion::new(8, 3),
    };
    let s = serde_json::to_string(&original).unwrap();
    assert_eq!(s, r#"{"name":"alpha","version":"8.3"}"#);

    let back: IpcSetPhp = serde_json::from_str(&s).unwrap();
    assert_eq!(back, original);
}
