//! Frozen-shape contract for `HelperInvocation::to_argv()`.
//!
//! These golden assertions pin the argv vectors `yerd-helper` will
//! receive. Adding a field, reordering, or renaming a flag trips the
//! test - that's exactly what the wire contract is for.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsString;
use std::path::PathBuf;

use yerd_platform::{CaFingerprint, HelperInvocation};

fn argv_strs(inv: &HelperInvocation) -> Vec<String> {
    inv.to_argv()
        .into_iter()
        .map(|o: OsString| o.into_string().unwrap())
        .collect()
}

#[test]
fn install_ca_argv_frozen() {
    let inv = HelperInvocation::InstallCa {
        ca_pem_path: PathBuf::from("/run/user/1000/yerd/ca.pem"),
        fp: CaFingerprint::new([0xAB; 32]),
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "install-ca".to_string(),
            "--pem".to_string(),
            "/run/user/1000/yerd/ca.pem".to_string(),
            "--fingerprint".to_string(),
            "ab".repeat(32),
        ]
    );
}

#[test]
fn uninstall_ca_argv_frozen() {
    let inv = HelperInvocation::UninstallCa {
        fp: CaFingerprint::new([0x12; 32]),
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "uninstall-ca".to_string(),
            "--fingerprint".to_string(),
            "12".repeat(32),
        ]
    );
}

#[test]
fn install_resolver_argv_frozen() {
    let inv = HelperInvocation::InstallResolver {
        tld: "test".to_string(),
        addr: "127.0.0.1:5353".parse().unwrap(),
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "install-resolver".to_string(),
            "--tld".to_string(),
            "test".to_string(),
            "--addr".to_string(),
            "127.0.0.1:5353".to_string(),
        ]
    );
}

#[test]
fn uninstall_resolver_argv_frozen() {
    let inv = HelperInvocation::UninstallResolver {
        tld: "test".to_string(),
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "uninstall-resolver".to_string(),
            "--tld".to_string(),
            "test".to_string(),
        ]
    );
}

#[test]
fn setcap_argv_frozen() {
    let inv = HelperInvocation::Setcap {
        daemon_binary: PathBuf::from("/usr/bin/yerdd"),
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "setcap".to_string(),
            "--binary".to_string(),
            "/usr/bin/yerdd".to_string(),
        ]
    );
}

#[test]
fn install_port_redirect_argv_frozen() {
    let inv = HelperInvocation::InstallPortRedirect {
        http_from: 80,
        http_to: 8080,
        https_from: 443,
        https_to: 8443,
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "install-port-redirect".to_string(),
            "--http-from".to_string(),
            "80".to_string(),
            "--http-to".to_string(),
            "8080".to_string(),
            "--https-from".to_string(),
            "443".to_string(),
            "--https-to".to_string(),
            "8443".to_string(),
        ]
    );
}

#[test]
fn uninstall_port_redirect_argv_frozen() {
    let inv = HelperInvocation::UninstallPortRedirect;
    assert_eq!(argv_strs(&inv), vec!["uninstall-port-redirect".to_string()]);
}

#[test]
fn install_lan_port_redirect_argv_frozen() {
    let inv = HelperInvocation::InstallLanPortRedirect {
        lan_ip: "192.168.1.42".parse().unwrap(),
        http_from: 80,
        http_to: 8080,
        https_from: 443,
        https_to: 8443,
    };
    assert_eq!(
        argv_strs(&inv),
        vec![
            "install-lan-port-redirect".to_string(),
            "--lan-ip".to_string(),
            "192.168.1.42".to_string(),
            "--http-from".to_string(),
            "80".to_string(),
            "--http-to".to_string(),
            "8080".to_string(),
            "--https-from".to_string(),
            "443".to_string(),
            "--https-to".to_string(),
            "8443".to_string(),
        ]
    );
}

#[test]
fn uninstall_lan_port_redirect_argv_frozen() {
    let inv = HelperInvocation::UninstallLanPortRedirect;
    assert_eq!(
        argv_strs(&inv),
        vec!["uninstall-lan-port-redirect".to_string()]
    );
}

#[test]
fn lan_port_redirect_round_trips_through_argv() {
    for inv in [
        HelperInvocation::InstallLanPortRedirect {
            lan_ip: "10.1.2.3".parse().unwrap(),
            http_from: 80,
            http_to: 8080,
            https_from: 443,
            https_to: 8443,
        },
        HelperInvocation::UninstallLanPortRedirect,
    ] {
        let argv = inv.to_argv();
        // Re-serialising the parsed invocation reproduces the argv byte-for-byte.
        let reparsed = HelperInvocation::from_argv(&argv).unwrap();
        assert_eq!(reparsed.to_argv(), argv);
    }
}

#[test]
fn fingerprint_in_argv_is_64_lowercase_hex() {
    let inv = HelperInvocation::UninstallCa {
        fp: CaFingerprint::new([0xFF; 32]),
    };
    let v = argv_strs(&inv);
    let fp_str = &v[2];
    assert_eq!(fp_str.len(), 64);
    assert!(fp_str.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(fp_str.chars().all(|c| !c.is_ascii_uppercase()));
}

#[test]
fn first_argv_element_is_always_the_op_tag() {
    let pairs: &[(HelperInvocation, &str)] = &[
        (
            HelperInvocation::InstallCa {
                ca_pem_path: PathBuf::from("/x"),
                fp: CaFingerprint::new([0; 32]),
            },
            "install-ca",
        ),
        (
            HelperInvocation::UninstallCa {
                fp: CaFingerprint::new([0; 32]),
            },
            "uninstall-ca",
        ),
        (
            HelperInvocation::InstallResolver {
                tld: "t".into(),
                addr: "127.0.0.1:1".parse().unwrap(),
            },
            "install-resolver",
        ),
        (
            HelperInvocation::UninstallResolver { tld: "t".into() },
            "uninstall-resolver",
        ),
        (
            HelperInvocation::Setcap {
                daemon_binary: PathBuf::from("/x"),
            },
            "setcap",
        ),
        (
            HelperInvocation::InstallPortRedirect {
                http_from: 80,
                http_to: 8080,
                https_from: 443,
                https_to: 8443,
            },
            "install-port-redirect",
        ),
        (
            HelperInvocation::UninstallPortRedirect,
            "uninstall-port-redirect",
        ),
    ];
    for (inv, expected) in pairs {
        let v = argv_strs(inv);
        assert_eq!(v[0], *expected);
    }
}
