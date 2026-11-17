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

use yerd_core::node::{MajorLine, NodeSource};
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

#[test]
fn node_source_serialises_as_kebab_case_string() {
    for (source, expected) in [
        (NodeSource::Nvmrc, r#""nvmrc""#),
        (NodeSource::NodeVersion, r#""node-version""#),
        (NodeSource::Site, r#""site""#),
        (NodeSource::Default, r#""default""#),
    ] {
        assert_eq!(serde_json::to_string(&source).unwrap(), expected);
    }
}

#[test]
fn major_line_byte_shape_frozen() {
    let full = MajorLine {
        major: 22,
        latest: "22.12.0".into(),
        codename: Some("Jod".into()),
        eol: Some(false),
        installed_latest: Some("22.9.0".into()),
        update_available: true,
    };
    assert_eq!(
        serde_json::to_string(&full).unwrap(),
        r#"{"major":22,"latest":"22.12.0","codename":"Jod","eol":false,"installed_latest":"22.9.0","update_available":true}"#
    );
    let bare = MajorLine {
        major: 25,
        latest: "25.1.0".into(),
        codename: None,
        eol: None,
        installed_latest: None,
        update_available: false,
    };
    assert_eq!(
        serde_json::to_string(&bare).unwrap(),
        r#"{"major":25,"latest":"25.1.0","update_available":false}"#
    );
}

#[test]
fn outbound_proxy_mode_serialises_as_snake_case_string() {
    use yerd_core::network::OutboundProxyMode;
    for (mode, expected) in [
        (OutboundProxyMode::System, r#""system""#),
        (OutboundProxyMode::Manual, r#""manual""#),
        (OutboundProxyMode::Off, r#""off""#),
    ] {
        assert_eq!(serde_json::to_string(&mode).unwrap(), expected);
        assert_eq!(
            serde_json::from_str::<OutboundProxyMode>(expected).unwrap(),
            mode
        );
    }
}

#[test]
fn proxy_source_serialises_as_snake_case_string() {
    use yerd_core::network::ProxySource;
    for (source, expected) in [
        (ProxySource::Manual, r#""manual""#),
        (ProxySource::DaemonEnv, r#""daemon_env""#),
        (
            ProxySource::MacosSystemSettings,
            r#""macos_system_settings""#,
        ),
        (ProxySource::GnomeSettings, r#""gnome_settings""#),
        (ProxySource::LoginShell, r#""login_shell""#),
        (ProxySource::None, r#""none""#),
    ] {
        assert_eq!(serde_json::to_string(&source).unwrap(), expected);
        assert_eq!(
            serde_json::from_str::<ProxySource>(expected).unwrap(),
            source
        );
    }
}
