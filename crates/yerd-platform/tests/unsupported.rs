//! Stub-only test: every trait method returns `Unsupported` on targets with no
//! real OS impl (i.e. not Linux, macOS, or Windows). Windows has a real `Paths`
//! impl, so it is covered by `windows_smoke.rs` instead.

#![cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{Ipv4Addr, SocketAddr};

mod common;

use yerd_platform::{
    ActiveIdeLauncher, ActivePaths, ActivePortBinder, ActiveResolverInstaller, ActiveSystemOpener,
    ActiveTerminalLauncher, ActiveTrustStore, DetectedIde, IdeLauncher, LaunchTarget, Paths,
    PlatformError, PortBinder, ResolverInstaller, SystemOpener, TerminalLauncher, TrustStore,
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
        ts.install_firefox_nss(std::path::Path::new("/ca.pem"))
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.uninstall_firefox_nss().unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.browser_ca_trust(&fp).unwrap_err(),
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
        r.is_installed("test", "127.0.0.1:1053".parse().unwrap())
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn terminal_launcher_unsupported() {
    assert!(matches!(
        ActiveTerminalLauncher
            .open_terminal(std::path::Path::new("/srv/site"))
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn ide_launcher_unsupported() {
    let launcher = ActiveIdeLauncher;
    assert!(launcher.detect().is_empty());
    let ide = DetectedIde {
        id: "vscode",
        display_name: "VS Code",
        launch: LaunchTarget::Cli(std::path::PathBuf::from("/usr/bin/code")),
    };
    assert!(matches!(
        launcher
            .launch(&ide, std::path::Path::new("/srv/site"))
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn system_opener_unsupported() {
    assert!(matches!(
        ActiveSystemOpener
            .open_path(std::path::Path::new("/srv/site"))
            .unwrap_err(),
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
        b.bind_pair(false, (0, 0), (0, 0)).unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}
