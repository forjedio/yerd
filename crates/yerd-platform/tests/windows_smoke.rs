//! Windows smoke test: the real impls (`WindowsPaths`, `WindowsPortBinder`)
//! resolve/bind, and every trait still aliased to the `unsupported` stub returns
//! `Unsupported`.

#![cfg(target_os = "windows")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{Ipv4Addr, SocketAddr};

mod common;

use yerd_platform::{
    ActivePaths, ActivePortBinder, ActiveResolverInstaller, ActiveTerminalLauncher,
    ActiveTrustStore, Paths, PlatformError, PortBinder, ResolverInstaller, TerminalLauncher,
    TrustStore,
};

use common::random_fingerprint;

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[test]
fn current_user_sid_resolves_and_pipe_name_is_derived() {
    let sid = yerd_platform::current_user_sid().expect("whoami resolves a SID");
    assert!(sid.starts_with("S-1-"), "{sid}");
    let dirs = ActivePaths::new().resolve().expect("resolve on Windows");
    let name = yerd_platform::daemon_pipe_name(&dirs).expect("derive pipe name");
    assert!(name.starts_with(&format!("yerd-{sid}-")), "{name}");
}

#[test]
fn paths_resolve_returns_yerd_layout() {
    let dirs = ActivePaths::new().resolve().expect("resolve on Windows");
    assert!(dirs.config.ends_with("yerd"), "{:?}", dirs.config);
    assert!(dirs.data.ends_with(r"yerd\data"), "{:?}", dirs.data);
    assert!(dirs.state.ends_with(r"yerd\state"), "{:?}", dirs.state);
    assert!(dirs.cache.ends_with(r"yerd\cache"), "{:?}", dirs.cache);
    assert!(dirs.runtime.ends_with("yerd"), "{:?}", dirs.runtime);
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
}

#[test]
fn terminal_launcher_unsupported() {
    assert!(matches!(
        ActiveTerminalLauncher
            .open_terminal(std::path::Path::new(r"C:\srv\site"))
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn port_binder_binds_ephemeral_and_reports_port() {
    let bound = ActivePortBinder.bind(0).expect("ephemeral loopback bind");
    let port = bound.port().expect("port readback");
    assert_ne!(port, 0, "an ephemeral bind must resolve a concrete port");
}

#[test]
fn port_binder_reports_addr_in_use_on_double_bind() {
    let first = ActivePortBinder.bind(0).expect("first bind");
    let port = first.port().expect("port readback");
    match ActivePortBinder.bind(port) {
        Err(PlatformError::Bind { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::AddrInUse, "{source:?}");
        }
        other => panic!("expected AddrInUse Bind error, got {other:?}"),
    }
}

#[test]
fn port_binder_bind_pair_keeps_desired_when_both_free() {
    let pair = ActivePortBinder
        .bind_pair(false, (0, 0), (0, 0))
        .expect("ephemeral pair binds");
    assert_ne!(pair.http.port().unwrap(), 0);
    assert_ne!(pair.https.port().unwrap(), 0);
}
