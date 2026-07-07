//! Shared `cargo metadata` dependency-graph assertions, used by every crate's
//! and binary's `tests/no_runtime_deps.rs` guard (`crates/yerd-tls`,
//! `crates/yerd-platform`, `crates/yerd-php`, `crates/yerd-proxy`,
//! `bin/yerdd`, `bin/yerd-helper`) to keep the workspace off OpenSSL/
//! native-tls and free of accidental diamond-dependency version splits.
//!
//! **Test-only.** This crate exists purely to be pulled in via
//! `[dev-dependencies]` - never `[dependencies]` - by the crates it checks.
//! That's load-bearing, not just convention: `cargo metadata`'s `dep_kinds`
//! marks a dev-dependency edge as `"dev"`, not `null` ("normal"), and the BFS
//! below only follows normal edges - so a consumer depending on this crate
//! for tests never pollutes its own runtime-graph assertions with this
//! crate's own dependencies (`serde_json`, already present everywhere as a
//! dev-dependency in its own right).
//!
//! Panics (via `unwrap`/`expect`) on any `cargo`/`rustc` invocation failure
//! or malformed `cargo metadata` output - appropriate for test-only
//! infrastructure that should fail loudly rather than silently pass an
//! incomplete graph.

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

/// The `--filter-platform`-scoped, *normal*-edges-only dependency graph
/// reachable from one workspace package, built once via `cargo metadata`.
pub struct DepGraph {
    /// `(name, version)` for every package on a normal (non-dev, non-build)
    /// dependency edge reachable from the root package, including the root
    /// itself.
    reachable: Vec<(String, String)>,
}

impl DepGraph {
    /// Run `cargo metadata --locked --filter-platform <host>` and breadth-
    /// first walk the *normal* dependency edges reachable from the workspace
    /// package named `root`.
    #[must_use]
    pub fn for_package(root: &str) -> Self {
        let meta = run_cargo_metadata();

        let mut pkg_name: HashMap<&str, &str> = HashMap::new();
        let mut pkg_version: HashMap<&str, &str> = HashMap::new();
        for p in meta["packages"].as_array().unwrap() {
            let id = p["id"].as_str().unwrap();
            pkg_name.insert(id, p["name"].as_str().unwrap());
            pkg_version.insert(id, p["version"].as_str().unwrap());
        }

        let mut nodes_by_id: HashMap<&str, &Value> = HashMap::new();
        for n in meta["resolve"]["nodes"].as_array().unwrap() {
            nodes_by_id.insert(n["id"].as_str().unwrap(), n);
        }

        let root_id = pkg_name.iter().find(|(_, n)| **n == root).map_or_else(
            || panic!("{root} must appear in cargo metadata"),
            |(id, _)| *id,
        );

        let mut reachable: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(root_id);
        reachable.insert(root_id);
        while let Some(id) = queue.pop_front() {
            let node = nodes_by_id[id];
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

        let reachable = reachable
            .into_iter()
            .map(|id| (pkg_name[id].to_owned(), pkg_version[id].to_owned()))
            .collect();
        Self { reachable }
    }

    /// Assert that none of `forbidden` appear anywhere in the reachable graph.
    pub fn assert_none_of(&self, forbidden: &[&str]) {
        for name in forbidden {
            assert!(
                !self.reachable.iter().any(|(n, _)| n == name),
                "{name} appeared in the runtime graph"
            );
        }
    }

    /// Assert each of `names` resolves to **at most one** version in the
    /// reachable graph (absent entirely is fine - use
    /// [`Self::assert_exactly_one_version_each`] to also require presence).
    pub fn assert_at_most_one_version_each(&self, names: &[&str]) {
        for name in names {
            let versions = self.versions_of(name);
            assert!(
                versions.len() <= 1,
                "expected at most one {name} version, found {versions:?}"
            );
        }
    }

    /// Assert each of `names` resolves to **exactly one** version - present,
    /// and present only once.
    pub fn assert_exactly_one_version_each(&self, names: &[&str]) {
        for name in names {
            let versions = self.versions_of(name);
            assert_eq!(
                versions.len(),
                1,
                "expected exactly one {name} version, found {versions:?}"
            );
        }
    }

    fn versions_of(&self, name: &str) -> HashSet<&str> {
        self.reachable
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// Best-effort current target triple, via `rustc -vV`'s `host:` line.
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
