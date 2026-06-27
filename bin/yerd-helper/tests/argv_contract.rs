//! Wire-contract cross-check: for every `HelperInvocation` variant,
//! `to_argv` produces a vector that the helper's own clap layer
//! consumes back into the same operation tag, and
//! `HelperInvocation::from_argv` agrees.
//!
//! This is the integration-level twin of the in-source debug-build
//! cross-check.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsString;
use std::path::PathBuf;

use yerd_platform::{CaFingerprint, HelperInvocation};

fn argv_for(inv: &HelperInvocation) -> Vec<OsString> {
    inv.to_argv()
}

fn op_tag(inv: &HelperInvocation) -> &'static str {
    use yerd_platform::error::ops;
    match inv {
        HelperInvocation::InstallCa { .. } => ops::INSTALL_CA,
        HelperInvocation::UninstallCa { .. } => ops::UNINSTALL_CA,
        HelperInvocation::InstallResolver { .. } => ops::INSTALL_RESOLVER,
        HelperInvocation::UninstallResolver { .. } => ops::UNINSTALL_RESOLVER,
        HelperInvocation::Setcap { .. } => ops::SETCAP,
        HelperInvocation::InstallPortRedirect { .. } => ops::INSTALL_PORT_REDIRECT,
        HelperInvocation::UninstallPortRedirect => ops::UNINSTALL_PORT_REDIRECT,
        _ => "unknown",
    }
}

#[test]
fn every_variant_round_trips_via_from_argv() {
    let cases: &[HelperInvocation] = &[
        HelperInvocation::InstallCa {
            ca_pem_path: PathBuf::from("/run/yerd/ca.pem"),
            fp: CaFingerprint::new([0xAB; 32]),
        },
        HelperInvocation::UninstallCa {
            fp: CaFingerprint::new([0x12; 32]),
        },
        HelperInvocation::InstallResolver {
            tld: "test".into(),
            addr: "127.0.0.1:5353".parse().unwrap(),
        },
        HelperInvocation::UninstallResolver { tld: "test".into() },
        HelperInvocation::Setcap {
            daemon_binary: PathBuf::from("/usr/bin/yerdd"),
        },
        HelperInvocation::InstallPortRedirect {
            http_from: 80,
            http_to: 8080,
            https_from: 443,
            https_to: 8443,
        },
        HelperInvocation::UninstallPortRedirect,
    ];
    for inv in cases {
        let v = argv_for(inv);
        let parsed = HelperInvocation::from_argv(&v).expect("from_argv must accept to_argv");
        assert_eq!(
            op_tag(&parsed),
            op_tag(inv),
            "round-trip op tag mismatch for {inv:?}"
        );
        assert_eq!(parsed.to_argv(), v, "round-trip argv mismatch for {inv:?}");
    }
}
