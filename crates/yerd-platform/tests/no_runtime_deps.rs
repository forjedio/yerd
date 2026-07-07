//! Dep-graph invariant: nothing in `yerd-platform`'s own runtime graph
//! drags in `tokio`, `anyhow`, `reqwest`, or any OpenSSL/native-tls
//! variant. `tokio` already exists elsewhere in the workspace (via
//! `yerd-ipc`'s transport feature), so this assertion is scoped to
//! `yerd-platform`'s own reachable set. See [`yerd_depcheck`] for the
//! shared `cargo metadata` walk.

#[test]
fn no_forbidden_crates_in_runtime_graph() {
    yerd_depcheck::DepGraph::for_package("yerd-platform").assert_none_of(&[
        "tokio",
        "anyhow",
        "reqwest",
        "openssl",
        "openssl-sys",
        "native-tls",
    ]);
}
