//! Dep-graph invariant: `yerd-php`'s default-features runtime graph must
//! not pull `anyhow`, `reqwest`, or any OpenSSL/native-tls variant.
//! `tokio` is allowed (the supervisor is intrinsically async). See
//! [`yerd_depcheck`] for the shared `cargo metadata` walk.

#[test]
fn no_forbidden_crates_in_runtime_graph_and_unique_versions() {
    let graph = yerd_depcheck::DepGraph::for_package("yerd-php");
    graph.assert_none_of(&["anyhow", "reqwest", "openssl", "openssl-sys", "native-tls"]);
    graph.assert_at_most_one_version_each(&["tokio", "time"]);
}
