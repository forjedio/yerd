//! Windows smoke test: the real impls (`WindowsPaths`, `WindowsPortBinder`,
//! `WindowsPortRedirector`, `WindowsTrustStore`) resolve/bind/probe over their
//! public API, while the traits still aliased to the `unsupported` stub
//! (resolver, terminal) return `Unsupported`. The trust probes here are
//! read-only against the real `CurrentUser` Root store (no confirmation dialog);
//! the hermetic `Memory`-store add/find/delete round-trip lives as a unit test
//! in `os::windows`.

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
    ActivePaths, ActivePortBinder, ActivePortRedirector, ActiveResolverInstaller,
    ActiveTerminalLauncher, ActiveTrustStore, Paths, PlatformError, PortBinder, PortRedirector,
    ResolverInstaller, TerminalLauncher, TrustStore,
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
fn trust_probe_reports_absent_for_random_fp() {
    let ts = ActiveTrustStore::new();
    let fp = random_fingerprint(0xCC);
    assert!(
        !ts.is_present_system(&fp)
            .expect("read-only `CurrentUser` Root probe"),
        "a random fingerprint is not in the user Root store"
    );
    assert!(
        !ts.is_trusted(std::path::Path::new("unused"), &fp)
            .expect("is_trusted delegates to the presence probe"),
        "presence is trust on Windows; a random fingerprint is not trusted"
    );
}

#[test]
fn trust_nss_methods_unsupported_on_windows() {
    let ts = ActiveTrustStore::new();
    assert!(matches!(
        ts.install_firefox_nss(std::path::Path::new("x"))
            .unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
    assert!(matches!(
        ts.uninstall_firefox_nss().unwrap_err(),
        PlatformError::Unsupported { .. }
    ));
}

#[test]
fn system_root_bundle_returns_public_roots() {
    let bundle = ActiveTrustStore::new()
        .system_root_bundle()
        .expect("enumerate host Root stores");
    let pem = bundle.expect("a real Windows host has populated Root stores");
    assert!(
        pem.contains("BEGIN CERTIFICATE"),
        "bundle must contain at least one root"
    );
}

#[test]
fn port_redirector_is_na_but_probes_foreign_listener() {
    let r = ActivePortRedirector::new();
    assert_eq!(
        r.is_active(),
        None,
        "Windows direct-binds 80/443; there is no redirect to be active"
    );
    assert!(
        r.foreign_web_listener().is_some(),
        "the trait-default loopback probe applies on Windows"
    );
    assert_eq!(
        r.redirect_targets(),
        None,
        "pf anchors are macOS-only; Windows has no redirect targets"
    );
}

#[test]
fn resolver_install_uninstall_need_helper() {
    let r = ActiveResolverInstaller::new();
    assert!(matches!(
        r.install("test", loopback(53)).unwrap_err(),
        PlatformError::NeedsHelper { .. }
    ));
    assert!(matches!(
        r.uninstall("test").unwrap_err(),
        PlatformError::NeedsHelper { .. }
    ));
}

#[test]
fn resolver_is_installed_reads_registry_without_error() {
    let r = ActiveResolverInstaller::new();
    assert!(
        r.is_installed("test", loopback(53)).is_ok(),
        "the read-only NRPT probe must never error (key-absent is Ok(false))"
    );
}

#[test]
fn resolver_is_installed_false_for_non_53_port() {
    let r = ActiveResolverInstaller::new();
    assert!(
        !r.is_installed("test", loopback(1053)).unwrap(),
        "an NRPT rule carries no port, so a non-53 addr can never be installed"
    );
    assert!(!r.is_installed("test", loopback(5353)).unwrap());
}

#[test]
fn resolver_is_installed_false_for_ipv6_addr() {
    let r = ActiveResolverInstaller::new();
    let v6 = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), 53);
    assert!(!r.is_installed("test", v6).unwrap());
}

#[test]
fn is_token_elevated_returns_a_bool() {
    let _ = yerd_platform::is_token_elevated();
}

#[test]
fn nrpt_guids_for_tld_reads_registry_without_panicking() {
    let _ = yerd_platform::nrpt_guids_for_tld("test");
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
