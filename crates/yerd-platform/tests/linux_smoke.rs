//! Per-OS smoke tests gated to Linux.
//!
//! Verifies the unprivileged surface: `Paths::resolve`, probes,
//! `PortBinder::bind`, and the `bind_pair` fallback codepath.

#![cfg(target_os = "linux")]
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
    let dirs = p.resolve().expect("Paths::resolve should succeed on Linux");
    assert!(!dirs.config.as_os_str().is_empty());
    assert!(!dirs.data.as_os_str().is_empty());
    assert!(!dirs.state.as_os_str().is_empty());
    assert!(!dirs.cache.as_os_str().is_empty());
    assert!(!dirs.runtime.as_os_str().is_empty());
}

#[test]
fn linux_runtime_and_state_are_distinct() {
    let p = ActivePaths;
    let dirs = p.resolve().expect("Linux Paths::resolve should succeed");
    assert_ne!(
        dirs.runtime, dirs.state,
        "runtime and state must never collapse on Linux"
    );
}

#[test]
fn install_system_returns_needs_helper() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xAA);
    let err = ts
        .install_system("pem", &fp)
        .expect_err("system install must require helper");
    assert!(matches!(err, PlatformError::NeedsHelper { operation } if operation == "install-ca"));
}

#[test]
fn uninstall_system_returns_needs_helper() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xBB);
    let err = ts
        .uninstall_system(&fp)
        .expect_err("system uninstall must require helper");
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "uninstall-ca"
    ));
}

/// Reading the host CA bundle must not error. The `ca-certificates` package
/// (present on CI) makes a bundle file exist; a minimal container without it is
/// tolerated by only requiring no error and, when `Some`, real cert content.
#[test]
fn system_root_bundle_reads_host_roots_without_error() {
    let ts = ActiveTrustStore;
    let out = ts
        .system_root_bundle()
        .expect("reading the host CA bundle must not error");
    if let Some(pem) = out {
        assert!(
            pem.contains("-----BEGIN CERTIFICATE-----"),
            "host root bundle should contain certificates"
        );
    }
}

#[test]
fn is_present_system_returns_false_for_random_fingerprint() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0x42);
    match ts.is_present_system(&fp) {
        Ok(present) => assert!(!present, "random fingerprint must not match"),
        Err(PlatformError::TrustStore { .. }) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn resolver_install_returns_needs_helper() {
    let r = ActiveResolverInstaller;
    let err = r.install("test", loopback(5353)).unwrap_err();
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "install-resolver"
    ));
}

#[test]
fn resolver_uninstall_returns_needs_helper() {
    let r = ActiveResolverInstaller;
    let err = r.uninstall("test").unwrap_err();
    assert!(matches!(
        err,
        PlatformError::NeedsHelper { operation } if operation == "uninstall-resolver"
    ));
}

#[test]
fn resolver_is_installed_returns_false_for_unknown_tld() {
    let r = ActiveResolverInstaller;
    let addr = "127.0.0.1:1053".parse().unwrap();
    assert!(!r.is_installed("yerd-unlikely-tld-xyz", addr).unwrap());
}

#[test]
fn resolver_empty_tld_returns_resolver_error() {
    let r = ActiveResolverInstaller;
    let err = r.install("", loopback(53)).unwrap_err();
    assert!(matches!(err, PlatformError::Resolver { .. }));
}

#[test]
fn port_binder_bind_zero_yields_nonzero_port() {
    let b = ActivePortBinder;
    let port = b.bind(0).expect("bind(0) on loopback should succeed");
    let resolved = port.port().expect("local_addr should succeed");
    assert!(resolved != 0, "kernel-assigned port must be non-zero");
}

#[test]
fn port_binder_bind_pair_zero_pair_returns_two_distinct_ports() {
    let b = ActivePortBinder;
    let pair = b
        .bind_pair((0, 0), (0, 0))
        .expect("bind_pair on (0,0)/(0,0) should succeed");
    let http_port = pair.http.port().unwrap();
    let https_port = pair.https.port().unwrap();
    assert_ne!(http_port, 0);
    assert_ne!(https_port, 0);
    assert_ne!(http_port, https_port);
}

#[test]
fn port_binder_bind_pair_falls_back_when_desired_is_occupied() {
    let b = ActivePortBinder;

    let sacrifice = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied = sacrifice.local_addr().unwrap().port();

    let pair = b
        .bind_pair((occupied, 0), (0, 0))
        .expect("bind_pair must fall back when desired http is in use");

    let http_port = pair.http.port().unwrap();
    let https_port = pair.https.port().unwrap();
    assert_ne!(
        http_port, occupied,
        "fallback http must not reuse the occupied port"
    );
    assert_ne!(http_port, 0);
    assert_ne!(https_port, 0);

    drop(sacrifice);
}
