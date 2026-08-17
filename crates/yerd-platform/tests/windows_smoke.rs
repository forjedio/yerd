//! Windows smoke test: the real impls (`WindowsPaths`, `WindowsPortBinder`,
//! `WindowsPortRedirector`, `WindowsTrustStore`, `WindowsResolverInstaller`,
//! `WindowsTerminalLauncher`, `WindowsSystemOpener`, `WindowsSystemMetrics`,
//! `WindowsIdeLauncher`) resolve/bind/probe over their public API. No trait
//! aliases the `unsupported` stub on Windows any more. The trust probes here
//! are
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
    ActiveIdeLauncher, ActivePaths, ActivePortBinder, ActivePortRedirector,
    ActiveResolverInstaller, ActiveSystemMetrics, ActiveSystemOpener, ActiveTerminalLauncher,
    ActiveTrustStore, DetectedIde, IdeErrorReason, IdeLauncher, LaunchTarget, Paths, PlatformError,
    PortBinder, PortRedirector, ResolverInstaller, SystemMetrics, SystemOpener, TerminalLauncher,
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

/// `user_path` is a read-only HKCU probe: it must never panic, whether or not
/// the value is present. (`set_user_path` mutates the real `HKCU\Environment\Path`
/// and so is exercised manually, never in CI.)
#[test]
fn user_path_reads_without_panicking() {
    let _ = yerd_platform::user_path();
}

/// The Windows terminal launcher is a real impl. Its spawn path
/// opens a real console window, so it is exercised manually via the GUI, not in
/// CI; here we only pin that the active type is constructible and implements the
/// trait (the pure probe shapes are unit-tested in `pure::win_terminal`).
#[test]
fn terminal_launcher_is_constructible_and_implements_the_trait() {
    fn assert_impl<T: TerminalLauncher>(_: &T) {}
    let launcher = ActiveTerminalLauncher::new();
    assert_impl(&launcher);
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

/// `system32_exe` must compose an absolute path under `System32`, never a bare
/// name that could resolve off `PATH`. Relocated here from `bin/yerd`, whose
/// local copy this crate's canonical definition replaced.
#[test]
fn system32_exe_is_absolute_under_system32() {
    let p = yerd_platform::system32_exe("taskkill.exe");
    assert!(p.ends_with(r"System32\taskkill.exe"), "{p:?}");
    assert!(p.is_absolute(), "{p:?}");
}

/// The Windows system opener is a real impl. Calling `open_path` would pop a
/// real Explorer window on the runner, the same reason the terminal launcher's
/// spawn is excluded here, so this only pins that the active type is
/// constructible and implements the trait.
#[test]
fn system_opener_is_constructible_and_implements_the_trait() {
    fn assert_impl<T: SystemOpener>(_: &T) {}
    let opener = ActiveSystemOpener::new();
    assert_impl(&opener);
}

/// The test process is certainly resident, so this is a genuine end-to-end
/// assertion that the `tasklist` spawn and the pure parser agree on the real
/// output shape.
#[test]
fn rss_bytes_reports_the_current_process() {
    let metrics = ActiveSystemMetrics::new();
    let rss = metrics.rss_bytes(std::process::id());
    assert!(rss.is_some_and(|n| n > 0), "got {rss:?}");
}

#[test]
fn rss_bytes_is_none_for_a_nonexistent_pid() {
    assert_eq!(ActiveSystemMetrics::new().rss_bytes(u32::MAX), None);
}

/// Windows has no load-average concept, the same answer macOS gives.
#[test]
fn load_average_is_none() {
    assert_eq!(ActiveSystemMetrics::new().load_average(), None);
}

/// Shape guard: whatever the runner happens to have installed, every detected
/// entry must be a real file with a known id, ordered by rank. Passes vacuously
/// on a runner with no editors, which is the expected CI case.
#[test]
fn detected_ides_are_well_formed() {
    let found = ActiveIdeLauncher::new().detect();
    let mut last_rank = 0u8;
    for ide in &found {
        let spec = yerd_platform::pure::ide_spec::spec_for(ide.id).expect("id is in IDE_SPECS");
        let LaunchTarget::Cli(ref exe) = ide.launch else {
            panic!(
                "windows detection must only yield Cli targets, got {:?}",
                ide.launch
            );
        };
        assert!(exe.is_file(), "{exe:?} is not a file");
        assert!(spec.rank >= last_rank, "detect() must be ordered by rank");
        last_rank = spec.rank;
    }
}

/// A target that does not exist must surface as a typed launch error, not a
/// panic or a silent success.
#[test]
fn launching_a_missing_executable_is_a_typed_error() {
    let ide = DetectedIde {
        id: "vscode",
        display_name: "Visual Studio Code",
        launch: LaunchTarget::Cli(std::path::PathBuf::from(r"C:\yerd-does-not-exist\nope.exe")),
    };
    let dir = tempfile::tempdir().unwrap();
    let err = ActiveIdeLauncher::new()
        .launch(&ide, dir.path())
        .unwrap_err();
    assert!(
        matches!(
            err,
            PlatformError::Ide {
                reason: IdeErrorReason::Launch { .. }
            }
        ),
        "got {err:?}"
    );
}

/// End-to-end proof that `std` runs a `.cmd` shim: the Toolbox and VS Code
/// launchers are batch files, so this is the behaviour the whole adapter rests
/// on. The shim writes a marker file we then poll for.
#[test]
fn a_cmd_shim_is_actually_launched() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("launched.txt");
    let shim = dir.path().join("marker.cmd");
    std::fs::write(&shim, format!("@echo done> \"{}\"\r\n", marker.display())).unwrap();

    let ide = DetectedIde {
        id: "vscode",
        display_name: "Visual Studio Code",
        launch: LaunchTarget::Cli(shim),
    };
    ActiveIdeLauncher::new().launch(&ide, dir.path()).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !marker.is_file() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(marker.is_file(), "the .cmd shim did not run");
}
