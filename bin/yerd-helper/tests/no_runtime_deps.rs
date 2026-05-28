//! Dep-graph invariant: nothing in `yerd-helper`'s own runtime graph
//! drags in `tokio`, `reqwest`, or any OpenSSL/native-tls variant.
//! `anyhow` is allowed in binaries per project convention.
//!
//! Modelled on the equivalent test in `yerd-platform`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::process::Command;

use serde_json::Value;

const FORBIDDEN: &[&str] = &["tokio", "reqwest", "openssl", "openssl-sys", "native-tls"];

fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn current_target() -> String {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("rustc should be invocable");
    let s = String::from_utf8(out.stdout).expect("rustc -vV emits UTF-8");
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("host: ") {
            return rest.trim().to_string();
        }
    }
    panic!("rustc -vV did not include a host: line");
}

fn run_cargo_metadata() -> Value {
    let target = current_target();
    let output = Command::new(cargo_bin())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            &target,
        ])
        .output()
        .expect("cargo metadata should be invocable from a cargo-test process");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("cargo metadata exited non-zero. stderr:\n{stderr}");
    }
    serde_json::from_slice(&output.stdout).expect("cargo metadata emits valid JSON")
}

#[test]
fn no_forbidden_crates_in_runtime_graph() {
    let meta = run_cargo_metadata();

    let mut pkg_name: HashMap<&str, &str> = HashMap::new();
    for p in meta["packages"].as_array().unwrap() {
        let id = p["id"].as_str().unwrap();
        let name = p["name"].as_str().unwrap();
        pkg_name.insert(id, name);
    }

    let mut nodes_by_id: HashMap<&str, &Value> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().unwrap() {
        nodes_by_id.insert(n["id"].as_str().unwrap(), n);
    }

    let yerd_helper_id = pkg_name
        .iter()
        .find(|(_, n)| **n == "yerd-helper")
        .map(|(id, _)| *id)
        .expect("yerd-helper must appear in cargo metadata");

    let mut reachable: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(yerd_helper_id);
    reachable.insert(yerd_helper_id);
    while let Some(id) = queue.pop_front() {
        let node = nodes_by_id.get(id).copied().unwrap();
        for dep in node["deps"].as_array().unwrap() {
            let kinds = dep["dep_kinds"].as_array().unwrap();
            let is_normal = kinds.iter().any(|k| k["kind"].is_null());
            if !is_normal {
                continue;
            }
            let pkg = dep["pkg"].as_str().unwrap();
            if reachable.insert(pkg) {
                queue.push_back(pkg);
            }
        }
    }

    let reachable_names: HashSet<&str> = reachable
        .iter()
        .filter_map(|id| pkg_name.get(id).copied())
        .collect();

    for forbidden in FORBIDDEN {
        assert!(
            !reachable_names.contains(forbidden),
            "{forbidden} appeared in yerd-helper's runtime graph"
        );
    }
}
