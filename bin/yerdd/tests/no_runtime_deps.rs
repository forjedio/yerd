//! Dep-graph invariant: `yerdd`'s default-features runtime graph must
//! not pull `anyhow`, OpenSSL / native-tls family, `hyper-tls`,
//! `tokio-native-tls`, or `fs2` (deprecated).
//!
//! `webpki-roots` IS allowed: the daemon fetches prebuilt PHP over HTTPS
//! (`reqwest` + rustls) for `yerd install php`, and bundled Mozilla roots are
//! the right trust anchor for a client hitting a public host (no OpenSSL).
//! See [`yerd_depcheck`] for the shared `cargo metadata` walk.

#[test]
fn no_forbidden_crates_in_runtime_graph_and_unique_versions() {
    let graph = yerd_depcheck::DepGraph::for_package("yerdd");
    graph.assert_none_of(&[
        "anyhow",
        "openssl",
        "openssl-sys",
        "native-tls",
        "hyper-tls",
        "tokio-native-tls",
        "fs2",
    ]);
    graph.assert_at_most_one_version_each(&["hyper", "rustls", "tokio", "time"]);
}
