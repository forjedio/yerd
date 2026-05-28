//! Stub-only test: every trait method returns `Unsupported` on
//! non-Linux, non-macOS targets (Phase 1: Windows).

#![cfg(not(any(target_os = "linux", target_os = "macos")))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
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
fn paths_resolve_unsupported() {
    let err = ActivePaths.resolve().unwrap_err();
    assert!(matches!(err, PlatformError::Unsupported { .. }));
}

#[test]
fn trust_store_unsupported() {
    let ts = ActiveTrustStore;
    let fp = random_fingerprint(0xCC);
    assert!(matches!(
        ts.install_system("p", &fp).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.uninstall_system(&fp).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.is_present_system(&fp).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.install_firefox_nss("p").unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn resolver_unsupported() {
    let r = ActiveResolverInstaller;
    assert!(matches!(
        r.install("test", loopback(53)).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        r.uninstall("test").unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        r.is_installed("test").unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn port_binder_unsupported() {
    let b = ActivePortBinder;
    assert!(matches!(
        b.bind(0).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        b.bind_pair((0, 0), (0, 0)).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}
