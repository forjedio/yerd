//! Dep-graph invariant: `yerdd`'s default-features runtime graph must
//! not pull `anyhow`, OpenSSL / native-tls family, `hyper-tls`,
//! `tokio-native-tls`, `webpki-roots`, or `fs2` (deprecated).

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

const FORBIDDEN: &[&str] = &[
    "anyhow",
    "openssl",
    "openssl-sys",
    "native-tls",
    "hyper-tls",
    "tokio-native-tls",
    "webpki-roots",
    "fs2",
];

const UNIQUE_VERSIONS: &[&str] = &["hyper", "rustls", "tokio", "time"];

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
fn no_forbidden_crates_in_runtime_graph_and_unique_versions() {
    let meta = run_cargo_metadata();

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

    let yerdd_id = pkg_name
        .iter()
        .find(|(_, n)| **n == "yerdd")
        .map(|(id, _)| *id)
        .expect("yerdd must appear in cargo metadata");

    let mut reachable: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(yerdd_id);
    reachable.insert(yerdd_id);
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

    let reachable_pairs: Vec<(&str, &str)> = reachable
        .iter()
        .map(|id| (pkg_name[id], pkg_version[id]))
        .collect();
    let reachable_names: HashSet<&str> = reachable_pairs.iter().map(|(n, _)| *n).collect();

    for forbidden in FORBIDDEN {
        assert!(
            !reachable_names.contains(forbidden),
            "{forbidden} appeared in yerdd's runtime graph"
        );
    }

    for unique in UNIQUE_VERSIONS {
        let versions: HashSet<&str> = reachable_pairs
            .iter()
            .filter(|(n, _)| n == unique)
            .map(|(_, v)| *v)
            .collect();
        assert!(
            versions.len() <= 1,
            "expected at most one {unique} version, found {versions:?}"
        );
    }
}
