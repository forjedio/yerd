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
        Self {
            reachable: reachable(&run_cargo_metadata(), root),
        }
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

/// The `(name, version)` set reachable from `root` over **normal**
/// (non-dev, non-build) dependency edges in a `cargo metadata` document,
/// including `root` itself. Split out of [`DepGraph::for_package`] so the BFS
/// and edge-filtering can be tested against synthetic metadata, independent of
/// real cargo/toolchain output.
fn reachable(meta: &Value, root: &str) -> Vec<(String, String)> {
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

    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(root_id);
    seen.insert(root_id);
    while let Some(id) = queue.pop_front() {
        let node = nodes_by_id[id];
        for dep in node["deps"].as_array().unwrap() {
            let kinds = dep["dep_kinds"].as_array().unwrap();
            let is_normal = kinds.iter().any(|k| k["kind"].is_null());
            if !is_normal {
                continue;
            }
            let pkg = dep["pkg"].as_str().unwrap();
            if seen.insert(pkg) {
                queue.push_back(pkg);
            }
        }
    }

    seen.into_iter()
        .map(|id| (pkg_name[id].to_owned(), pkg_version[id].to_owned()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `cargo metadata`-shaped document. `pkgs` is `(id, name, version)`;
    /// `edges` is `(from_id, to_id, kind)` where `kind` is `None` for a normal
    /// edge or `Some("dev")`/`Some("build")` for a filtered one.
    fn meta(pkgs: &[(&str, &str, &str)], edges: &[(&str, &str, Option<&str>)]) -> Value {
        let packages: Vec<Value> = pkgs
            .iter()
            .map(|(id, name, version)| json!({"id": id, "name": name, "version": version}))
            .collect();
        let nodes: Vec<Value> = pkgs
            .iter()
            .map(|(id, _, _)| {
                let deps: Vec<Value> = edges
                    .iter()
                    .filter(|(from, _, _)| from == id)
                    .map(|(_, to, kind)| json!({"pkg": to, "dep_kinds": [{"kind": kind}]}))
                    .collect();
                json!({"id": id, "deps": deps})
            })
            .collect();
        json!({"packages": packages, "resolve": {"nodes": nodes}})
    }

    fn names(mut r: Vec<(String, String)>) -> Vec<String> {
        r.sort();
        r.into_iter().map(|(n, _)| n).collect()
    }

    #[test]
    fn reachable_follows_normal_edges_transitively() {
        let m = meta(
            &[
                ("root", "root", "1.0.0"),
                ("a", "a", "1.0.0"),
                ("b", "b", "1.0.0"),
            ],
            &[("root", "a", None), ("a", "b", None)],
        );
        assert_eq!(names(reachable(&m, "root")), ["a", "b", "root"]);
    }

    #[test]
    fn reachable_skips_dev_and_build_edges() {
        let m = meta(
            &[
                ("root", "root", "1.0.0"),
                ("prod", "prod", "1.0.0"),
                ("devdep", "devdep", "1.0.0"),
                ("builddep", "builddep", "1.0.0"),
            ],
            &[
                ("root", "prod", None),
                ("root", "devdep", Some("dev")),
                ("root", "builddep", Some("build")),
            ],
        );
        assert_eq!(names(reachable(&m, "root")), ["prod", "root"]);
    }

    #[test]
    fn reachable_reports_both_versions_of_a_diamond_duplicate() {
        let m = meta(
            &[
                ("root", "root", "1.0.0"),
                ("a", "a", "1.0.0"),
                ("b", "b", "1.0.0"),
                ("dup1", "dup", "1.0.0"),
                ("dup2", "dup", "2.0.0"),
            ],
            &[
                ("root", "a", None),
                ("root", "b", None),
                ("a", "dup1", None),
                ("b", "dup2", None),
            ],
        );
        let graph = DepGraph {
            reachable: reachable(&m, "root"),
        };
        assert_eq!(graph.versions_of("dup").len(), 2);
    }

    #[test]
    #[should_panic(expected = "must appear in cargo metadata")]
    fn reachable_panics_when_root_absent() {
        let m = meta(&[("a", "a", "1.0.0")], &[]);
        let _ = reachable(&m, "nonexistent");
    }

    #[test]
    fn assert_at_most_one_version_passes_for_single_and_absent() {
        let graph = DepGraph {
            reachable: vec![("tokio".to_owned(), "1.0.0".to_owned())],
        };
        graph.assert_at_most_one_version_each(&["tokio", "not-present"]);
    }

    #[test]
    #[should_panic(expected = "expected at most one tokio version")]
    fn assert_at_most_one_version_fails_on_a_split() {
        let graph = DepGraph {
            reachable: vec![
                ("tokio".to_owned(), "1.0.0".to_owned()),
                ("tokio".to_owned(), "0.2.0".to_owned()),
            ],
        };
        graph.assert_at_most_one_version_each(&["tokio"]);
    }

    #[test]
    #[should_panic(expected = "appeared in the runtime graph")]
    fn assert_none_of_fails_when_a_forbidden_crate_is_reachable() {
        let graph = DepGraph {
            reachable: vec![("openssl".to_owned(), "0.10.0".to_owned())],
        };
        graph.assert_none_of(&["openssl"]);
    }
}
