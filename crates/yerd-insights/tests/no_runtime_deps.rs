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
//! asking. A reachability walk therefore cannot tell "yerd-insights pulled in tokio"
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

/// This crate's `cargo metadata` package entry.
fn package() -> Value {
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
    meta["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"] == "yerd-insights")
        .expect("yerd-insights must appear in cargo metadata")
        .clone()
}

/// Every runtime (non-dev, non-build) dependency this crate declares, as
/// `(name, enabled features)`.
fn runtime_dependencies() -> Vec<(String, Vec<String>)> {
    package()["dependencies"]
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
    for forbidden in ["tokio", "anyhow", "futures", "async-trait", "reqwest"] {
        assert!(
            !deps.iter().any(|(name, _)| name == forbidden),
            "{forbidden} must not be a runtime dependency of yerd-insights"
        );
    }
}

#[test]
fn declares_no_openssl() {
    let deps = runtime_dependencies();
    for (name, _) in &deps {
        assert!(
            !name.starts_with("openssl") && name != "native-tls",
            "{name} must not be a runtime dependency of yerd-insights"
        );
    }
}

#[test]
fn does_not_enable_yerd_ipc_transport() {
    let deps = runtime_dependencies();
    let (_, features) = deps
        .iter()
        .find(|(name, _)| name == "yerd-ipc")
        .expect("yerd-insights depends on yerd-ipc");
    assert!(
        !features.iter().any(|f| f == "transport"),
        "yerd-insights must use yerd-ipc's pure default build; `transport` pulls in tokio"
    );
}

/// The features in a `[features]` table that forward to `yerd-ipc/transport`.
fn transport_forwards(features: &Value) -> Vec<String> {
    features
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, enables)| {
            enables.as_array().is_some_and(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .any(|e| e == "yerd-ipc/transport" || e == "yerd-ipc?/transport")
            })
        })
        .map(|(name, _)| name.clone())
        .collect()
}

#[test]
fn transport_forwarding_is_detected() {
    let cases: &[(Value, &[&str])] = &[
        (serde_json::json!({}), &[]),
        (
            serde_json::json!({ "default": ["yerd-ipc/transport"] }),
            &["default"],
        ),
        (
            serde_json::json!({ "net": ["serde/std", "yerd-ipc?/transport"] }),
            &["net"],
        ),
        (serde_json::json!({ "default": ["yerd-ipc"] }), &[]),
    ];
    for (features, want) in cases {
        assert_eq!(transport_forwards(features), *want, "{features}");
    }
}

#[test]
fn no_feature_forwards_to_yerd_ipc_transport() {
    let forwards = transport_forwards(&package()["features"]);
    assert!(
        forwards.is_empty(),
        "features {forwards:?} enable yerd-ipc/transport, which pulls in tokio"
    );
}
