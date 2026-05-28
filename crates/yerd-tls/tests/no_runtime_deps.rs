//! Dep-graph invariants enforced via `cargo metadata`:
//!
//! - no `tokio` in the runtime graph
//! - no `anyhow` in the runtime graph
//! - exactly one `time` version
//! - exactly one `x509-parser` version
//!
//! Replaces the round-1 grep gate and the round-2 unreliable `cargo deny`
//! gate with a single deterministic test.

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

fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn run_cargo_metadata() -> Value {
    let output = Command::new(cargo_bin())
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .expect("cargo metadata should be invocable from a cargo-test process");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("cargo metadata exited non-zero. stderr:\n{stderr}");
    }
    serde_json::from_slice(&output.stdout).expect("cargo metadata emits valid JSON")
}

#[test]
fn no_tokio_or_anyhow_in_runtime_graph_and_pinned_versions_unique() {
    let meta = run_cargo_metadata();

    // Build id → package-name + id → resolve-node maps.
    let mut pkg_name: HashMap<&str, &str> = HashMap::new();
    let mut pkg_version: HashMap<&str, &str> = HashMap::new();
    for p in meta["packages"].as_array().unwrap() {
        let id = p["id"].as_str().unwrap();
        let name = p["name"].as_str().unwrap();
        let version = p["version"].as_str().unwrap();
        pkg_name.insert(id, name);
        pkg_version.insert(id, version);
    }

    let mut nodes_by_id: HashMap<&str, &Value> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().unwrap() {
        nodes_by_id.insert(n["id"].as_str().unwrap(), n);
    }

    // Locate yerd-tls's node id.
    let yerd_tls_id = pkg_name
        .iter()
        .find(|(_, n)| **n == "yerd-tls")
        .map(|(id, _)| *id)
        .expect("yerd-tls must appear in cargo metadata");

    // BFS over normal-kind edges only. Each `node.deps[i].dep_kinds[j].kind`
    // is `null` for normal runtime deps, `"dev"` for dev-deps, `"build"` for
    // build-deps. We scope to normal.
    let mut reachable: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(yerd_tls_id);
    reachable.insert(yerd_tls_id);
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

    // Collect (name, version) pairs from the reachable set.
    let mut reachable_pairs: Vec<(&str, &str)> = reachable
        .iter()
        .map(|id| (pkg_name[id], pkg_version[id]))
        .collect();
    reachable_pairs.sort_unstable();

    // (1) No tokio in the runtime graph.
    assert!(
        !reachable_pairs.iter().any(|(n, _)| *n == "tokio"),
        "tokio appeared in yerd-tls's runtime graph: {reachable_pairs:?}"
    );

    // (2) No anyhow in the runtime graph.
    assert!(
        !reachable_pairs.iter().any(|(n, _)| *n == "anyhow"),
        "anyhow appeared in yerd-tls's runtime graph"
    );

    // (3) Exactly one `time` version.
    let time_versions: HashSet<&str> = reachable_pairs
        .iter()
        .filter(|(n, _)| *n == "time")
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        time_versions.len(),
        1,
        "expected exactly one `time` version, found {time_versions:?}"
    );

    // (4) Exactly one `x509-parser` version.
    let x509_versions: HashSet<&str> = reachable_pairs
        .iter()
        .filter(|(n, _)| *n == "x509-parser")
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        x509_versions.len(),
        1,
        "expected exactly one `x509-parser` version, found {x509_versions:?}"
    );
}
