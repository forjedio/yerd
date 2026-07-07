//! Dep-graph invariants: no `tokio`, no `anyhow`, exactly one `time` version,
//! exactly one `x509-parser` version.
//!
//! Replaces the round-1 grep gate and the round-2 unreliable `cargo deny`
//! gate with a single deterministic test. See [`yerd_depcheck`] for the
//! shared `cargo metadata` walk this (and every other crate's own
//! `no_runtime_deps` test) is built on.

#[test]
fn no_tokio_or_anyhow_in_runtime_graph_and_pinned_versions_unique() {
    let graph = yerd_depcheck::DepGraph::for_package("yerd-tls");
    graph.assert_none_of(&["tokio", "anyhow"]);
    graph.assert_exactly_one_version_each(&["time", "x509-parser"]);
}
