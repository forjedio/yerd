# yerd-depcheck

`yerd-depcheck` is a small **test-only** crate: a shared `cargo metadata`
dependency-graph walk, used by every crate's and binary's
`tests/no_runtime_deps.rs` guard to keep the workspace off OpenSSL/native-tls
and free of accidental diamond-dependency version splits. It replaced six
near-identical ~140-line copies of the same breadth-first `cargo metadata`
walk (one per consumer) with one shared implementation.

::: info Crate metadata
`description`: *Test-only cargo-metadata dependency-graph assertions, shared
by every crate/binary's no_runtime_deps guard.* Depends only on `serde_json`.
Not `#![forbid(unsafe_code)]`-exempt in any way, but does carry a crate-level
`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, ...)]` -
the same allow every one of its six consumers' own `tests/no_runtime_deps.rs`
already needed, now centralised in one place instead of repeated six times.
:::

**Never appears in `[dependencies]`.** Every consumer pulls it in via
`[dev-dependencies]` only - that's load-bearing, not just convention:
`cargo metadata`'s `dep_kinds` marks a dev-dependency edge as `"dev"`, not
`null` ("normal"), and `DepGraph`'s own breadth-first walk only follows normal
edges. So a crate depending on `yerd-depcheck` for its tests never pollutes
its *own* runtime-graph assertions with `yerd-depcheck`'s dependencies.

## The API

```rust
pub struct DepGraph { /* private */ }

impl DepGraph {
    /// Run `cargo metadata --locked --filter-platform <host>` and BFS the
    /// normal dependency edges reachable from the workspace package `root`.
    pub fn for_package(root: &str) -> Self;

    /// Assert none of `forbidden` appear anywhere in the reachable graph.
    pub fn assert_none_of(&self, forbidden: &[&str]);

    /// Assert each of `names` resolves to at most one version (absent is fine).
    pub fn assert_at_most_one_version_each(&self, names: &[&str]);

    /// Assert each of `names` resolves to exactly one version (must be present).
    pub fn assert_exactly_one_version_each(&self, names: &[&str]);
}
```

A consumer's whole `tests/no_runtime_deps.rs` is now typically five to fifteen
lines:

```rust
#[test]
fn no_forbidden_crates_in_runtime_graph_and_unique_versions() {
    let graph = yerd_depcheck::DepGraph::for_package("yerd-proxy");
    graph.assert_none_of(&[
        "anyhow", "openssl", "openssl-sys", "native-tls",
        "hyper-tls", "tokio-native-tls", "webpki-roots",
    ]);
    graph.assert_at_most_one_version_each(&["hyper", "rustls", "tokio", "time"]);
}
```

## Consumers

| Crate/binary | Forbidden | Unique-version check |
|---|---|---|
| [`yerd-tls`](./yerd-tls) | `tokio`, `anyhow` | `time`, `x509-parser` (exactly one - both are known-present) |
| [`yerd-platform`](./yerd-platform) | `tokio`, `anyhow`, `reqwest`, `openssl`, `openssl-sys`, `native-tls` | - |
| [`yerd-php`](./yerd-php) | `anyhow`, `reqwest`, `openssl`, `openssl-sys`, `native-tls` | `tokio`, `time` (at most one) |
| [`yerd-proxy`](./yerd-proxy) | `anyhow`, `openssl`, `openssl-sys`, `native-tls`, `hyper-tls`, `tokio-native-tls`, `webpki-roots` | `hyper`, `rustls`, `tokio`, `time` (at most one) |
| [`yerdd`](../binaries/yerdd) | `anyhow`, `openssl`, `openssl-sys`, `native-tls`, `hyper-tls`, `tokio-native-tls`, `fs2` | `hyper`, `rustls`, `tokio`, `time` (at most one) |
| [`yerd-helper`](../binaries/yerd-helper) | `tokio`, `reqwest`, `openssl`, `openssl-sys`, `native-tls` | - |

Each consumer's forbidden list reflects *its own* dependency policy (e.g.
`yerdd` explicitly allows `webpki-roots`, since it fetches prebuilt PHP over
HTTPS via `reqwest` + rustls and bundled Mozilla roots are the right trust
anchor there; every other consumer forbids it). `yerd-depcheck` only supplies
the mechanism - each `tests/no_runtime_deps.rs` still states its own crate's
policy explicitly, so a `cargo test -p <crate> --test no_runtime_deps` failure
names exactly which crate's graph regressed.

::: info Standardised on `--filter-platform`
Before consolidation, five of the six consumers already ran `cargo metadata`
with `--filter-platform <host>` (scoping the graph to what actually compiles
for the current target); `yerd-tls`'s copy predated that pattern and queried
the full, unfiltered graph. `DepGraph::for_package` always filters - a strict
narrowing of the unfiltered graph, so this can only make a forbidden-crate or
unique-version assertion easier to satisfy, never spuriously fail one that
passed before.
:::

## See also

- [Crates Overview](../crates) - where this sits relative to the real
  (shipped) library crates.
- [Contributing](../contributing) - the full local test/lint gate this guard
  is part of.
- Source: [`crates/yerd-depcheck`](https://github.com/forjedio/yerd/tree/main/crates/yerd-depcheck)
