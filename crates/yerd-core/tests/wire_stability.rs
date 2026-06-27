//! Wire-stability gate for the IPC contract.
//!
//! Every assertion in this file pins a byte-exact JSON shape produced by
//! `yerd-core`'s public types. Renaming any public field, variant, or type
//! name fails this file - which fails CI before `yerd-ipc` sees a divergent
//! wire format.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use yerd_core::{PhpVersion, RouterConfig, Site, SiteKind};

#[test]
fn php_version_serialises_as_dotted_string() {
    let s = serde_json::to_string(&PhpVersion::new(8, 3)).unwrap();
    assert_eq!(s, r#""8.3""#);
}

#[test]
fn site_kind_serialises_as_snake_case_string() {
    assert_eq!(
        serde_json::to_string(&SiteKind::Parked).unwrap(),
        r#""parked""#
    );
    assert_eq!(
        serde_json::to_string(&SiteKind::Linked).unwrap(),
        r#""linked""#
    );
}

#[test]
fn canonical_site_byte_shape_frozen() {
    let s = Site::parked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
    let got = serde_json::to_string(&s).unwrap();
    let expected =
        r#"{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}"#;
    assert_eq!(got, expected);
}

#[test]
fn router_config_default_byte_shape_frozen() {
    let got = serde_json::to_string(&RouterConfig::default()).unwrap();
    assert_eq!(got, r#"{"tld":"test"}"#);
}
