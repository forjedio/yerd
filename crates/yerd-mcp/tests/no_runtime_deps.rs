//! Manifest invariant: this crate stays runtime-free.
//!
//! The risk being guarded is one manifest edit: `yerd-ipc` carries an optional
//! `transport` feature that pulls in tokio, so declaring
//! `yerd-ipc = { features = ["transport"] }` here (or adding tokio directly)
//! would drag an async runtime into a crate whose whole point is not to have
//! one - and nothing else would fail.
//!
//! Note this deliberately checks the *declared manifest* rather than using
//! [`yerd_depcheck::DepGraph`] like the other pure crates' guards. Those crates
//! (`yerd-tls`, `yerd-platform`, `yerd-php`) do not depend on `yerd-ipc`. This
//! one does, and `cargo metadata`'s resolve unifies features across the whole
//! workspace: because `yerdd` and `yerd` enable `yerd-ipc/transport`, the
//! resolved graph shows a normal `yerd-ipc -> tokio` edge no matter who is
//! asking. A reachability walk therefore cannot tell "yerd-mcp pulled in tokio"
//! from "a binary elsewhere in the workspace did", so it would fail here
//! whatever this crate declares. The manifest is the thing that actually
//! decides, so the manifest is what this asserts.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsString;
use std::process::Command;

use serde_json::Value;

/// Every runtime (non-dev, non-build) dependency this crate declares, as
/// `(name, enabled features)`.
fn runtime_dependencies() -> Vec<(String, Vec<String>)> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked", "--no-deps"])
        .output()
        .expect("cargo metadata should be invocable from a cargo-test process");
    assert!(
        output.status.success(),
        "cargo metadata exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let packages = meta["packages"].as_array().expect("packages array");
    let package = packages
        .iter()
        .find(|p| p["name"] == "yerd-mcp")
        .expect("yerd-mcp must appear in cargo metadata");

    package["dependencies"]
        .as_array()
        .expect("dependencies array")
        .iter()
        .filter(|d| d["kind"].is_null())
        .map(|d| {
            let name = d["name"].as_str().expect("dep name").to_owned();
            let features = d["features"]
                .as_array()
                .map(|f| {
                    f.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            (name, features)
        })
        .collect()
}

#[test]
fn declares_no_async_runtime_and_no_stringly_error_crate() {
    let deps = runtime_dependencies();
    for forbidden in ["tokio", "anyhow", "futures", "async-trait"] {
        assert!(
            !deps.iter().any(|(name, _)| name == forbidden),
            "{forbidden} must not be a runtime dependency of yerd-mcp"
        );
    }
}

#[test]
fn does_not_enable_yerd_ipc_transport() {
    let deps = runtime_dependencies();
    let (_, features) = deps
        .iter()
        .find(|(name, _)| name == "yerd-ipc")
        .expect("yerd-mcp depends on yerd-ipc");
    assert!(
        !features.iter().any(|f| f == "transport"),
        "yerd-mcp must use yerd-ipc's pure default build; `transport` pulls in tokio"
    );
}
