//! Per-OS smoke tests gated to Windows.

#![cfg(target_os = "windows")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::random_fingerprint;
use yerd_platform::{
    ActivePaths, ActivePortBinder, ActiveResolverInstaller, ActiveTrustStore, Paths, PlatformDirs,
    PlatformError, PortBinder, ResolverInstaller, TrustStore,
};

#[test]
fn paths_resolve_returns_windows_directories() {
    let dirs = ActivePaths.resolve().expect("Windows paths should resolve");
    assert!(!dirs.config.as_os_str().is_empty());
    assert!(!dirs.data.as_os_str().is_empty());
    assert!(!dirs.state.as_os_str().is_empty());
    assert!(!dirs.cache.as_os_str().is_empty());
    assert!(dirs.runtime.ends_with("run"));
}

#[test]
fn for_user_layout_matches_resolve_for_current_home() {
    let home = std::env::var_os("USERPROFILE").expect("USERPROFILE should exist");
    let resolved = ActivePaths.resolve().expect("Windows paths should resolve");
    let explicit = PlatformDirs::for_user(std::path::Path::new(&home), 0);
    assert_eq!(explicit, resolved);
}

#[test]
fn privileged_integrations_require_the_helper() {
    let fp = random_fingerprint(0xAA);
    assert!(matches!(
        ActiveTrustStore.install_system("pem", &fp),
        Err(PlatformError::NeedsHelper {
            operation: "install-ca"
        })
    ));
    assert!(matches!(
        ActiveResolverInstaller.install("test", "127.0.0.1:53".parse().unwrap()),
        Err(PlatformError::NeedsHelper {
            operation: "install-resolver"
        })
    ));
}

#[test]
fn port_binder_binds_single_and_pair() {
    let single = ActivePortBinder.bind(0).expect("bind ephemeral port");
    assert_ne!(single.port().unwrap(), 0);
    let pair = ActivePortBinder
        .bind_pair(false, (0, 0), (0, 0))
        .expect("bind ephemeral pair");
    assert_ne!(pair.http.port().unwrap(), pair.https.port().unwrap());
}

#[test]
fn port_binder_falls_back_from_occupied_port() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let pair = ActivePortBinder
        .bind_pair(false, (occupied_port, 0), (0, 0))
        .expect("occupied desired port should use fallback");
    assert_ne!(pair.http.port().unwrap(), occupied_port);
}
