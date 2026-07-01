//! Per-OS smoke tests gated to macOS. Same shape as `linux_smoke.rs`
//! but tolerates the macOS-specific runtime/state collapse and the
//! likely-absent `certutil`.

#![cfg(target_os = "macos")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::net::{Ipv4Addr, SocketAddr};

mod common;

use yerd_platform::{
    ActivePaths, ActivePortBinder, ActiveResolverInstaller, ActiveTrustStore, Paths, PlatformError,
    PortBinder, ResolverInstaller, TrustStore,
};

use common::random_fingerprint;

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[test]
fn paths_resolve_returns_all_five_fields() {
    let p = ActivePaths;
    let dirs = p.resolve().expect("Paths::resolve should succeed on macOS");
    assert!(!dirs.config.as_os_str().is_empty());
    assert!(!dirs.data.as_os_str().is_empty());
    assert!(!dirs.state.as_os_str().is_empty());
    assert!(!dirs.cache.as_os_str().is_empty());
    assert!(!dirs.runtime.as_os_str().is_empty());
}

#[test]
fn runtime_dir_is_deterministic_tmp_path() {
    let dirs = ActivePaths.resolve().expect("resolve should succeed");
    let s = dirs.runtime.to_string_lossy();
    assert!(
        s.starts_with("/tmp/yerd-"),
        "macOS runtime dir must be /tmp/yerd-$UID, got {s}"
    );
    assert!(
        s.trim_start_matches("/tmp/yerd-")
            .chars()
            .all(|c| c.is_ascii_digit()),
        "runtime dir must end in a numeric uid, got {s}"
    );
}

#[test]
fn install_system_returns_needs_helper() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xAA);
    let err = ts.install_system("pem", &fp).unwrap_err();
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "install-ca"
    ));
}

#[test]
fn uninstall_system_returns_needs_helper() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xBB);
    let err = ts.uninstall_system(&fp).unwrap_err();
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "uninstall-ca"
    ));
}

#[test]
fn is_trusted_errors_for_unreadable_cert() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xCC);
    let missing = std::path::Path::new("/tmp/yerd-nonexistent-ca-xyz.cert.pem");
    assert!(ts.is_trusted(missing, &fp).is_err());
}

/// macOS keychain enumeration yields the Apple public roots. Tolerates
/// `Ok(None)` (some CI runners can't reach the system root keychains, returning
/// errSecNoSuchKeychain), asserting content only when a bundle is produced -
/// mirroring `linux_smoke.rs`.
#[test]
fn system_root_bundle_returns_public_roots() {
    let ts = ActiveTrustStore;
    let out = ts
        .system_root_bundle()
        .expect("keychain enumeration should not error");
    if let Some(pem) = out {
        let count = pem.matches("-----BEGIN CERTIFICATE-----").count();
        assert!(
            count > 50,
            "expected a substantial set of Apple public roots, got {count}"
        );
    }
}

#[test]
fn resolver_install_returns_needs_helper() {
    let r = ActiveResolverInstaller;
    let err = r.install("test", loopback(53)).unwrap_err();
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "install-resolver"
    ));
}

#[test]
fn resolver_is_installed_returns_false_for_unknown_tld() {
    let r = ActiveResolverInstaller;
    let addr = "127.0.0.1:1053".parse().unwrap();
    assert!(!r.is_installed("yerd-unlikely-tld-xyz", addr).unwrap());
}

#[test]
fn port_binder_bind_zero_yields_nonzero_port() {
    let b = ActivePortBinder;
    let port = b.bind(0).expect("bind(0) on loopback should succeed");
    assert!(port.port().unwrap() != 0);
}

#[test]
fn port_binder_bind_pair_zero_pair_returns_two_distinct_ports() {
    let b = ActivePortBinder;
    let pair = b.bind_pair((0, 0), (0, 0)).unwrap();
    let http_port = pair.http.port().unwrap();
    let https_port = pair.https.port().unwrap();
    assert_ne!(http_port, https_port);
}
