//! Dep-graph invariant: the forwarder sits on every request when ports are
//! elevated, so its runtime graph stays tiny and free of async runtimes, HTTP
//! clients and TLS stacks. See [`yerd_depcheck`] for the shared walk.

#[test]
fn no_forbidden_crates_in_runtime_graph() {
    yerd_depcheck::DepGraph::for_package("yerd-forward").assert_none_of(&[
        "tokio",
        "reqwest",
        "openssl",
        "openssl-sys",
        "native-tls",
        "rustls",
    ]);
}
