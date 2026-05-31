# Yerd — agent instructions

These are the rules for any AI agent writing, modifying, or reviewing code in
this repository. Follow them exactly. Path-specific rules live in
`.github/instructions/*.instructions.md` and apply automatically to the files
they match; this file is the always-on baseline.

## What Yerd is

Yerd is a cross-platform local PHP development environment for **macOS, Linux,
and Windows**. It serves projects on `.test`
domains over HTTP/HTTPS, runs multiple PHP versions per site, and optionally
supervises databases and caches as native child processes. The product runs
**without root** in normal operation; setup may elevate once.

It is a Rust workspace plus a Tauri v2 + Vue 3 desktop app. macOS and Linux are
the supported platforms today; Windows support is planned and its OS adapters
are not yet implemented — do not assume Windows code paths exist.

## The single organising rule

> **Pure logic lives in library crates. I/O and OS calls are pushed to the
> edges behind traits.**

Everything else follows from this:

1. **Pure crates and pure modules do no I/O.** No filesystem, network, process
   spawning, clock reads, or environment reads. They must be unit-testable with
   in-memory fixtures and zero setup. `yerd-core` is the exemplar — keep it that
   way. Many crates split this physically into a `pure/` module (sync, no
   runtime) and an `io/` module (the side-effecting edge).
2. **Side effects go behind traits.** Anything touching the OS is a trait
   (`ProcessSpawner`, `TrustStore`, `ResolverInstaller`, `PortBinder`,
   `Downloader`, `Clock`, …). Business logic depends on the trait; tests inject
   a fake; the real implementation lives in `yerd-platform` or a crate's `os`
   module behind `#[cfg(...)]`.
3. **Binaries are thin.** `bin/yerdd`, `bin/yerd`, `bin/yerd-helper`, and the
   Tauri `src-tauri` layer wire crates together and own transports. They contain
   orchestration, not behaviour. Interesting logic belongs in a crate with tests.
4. **The IPC protocol is a stable contract.** Add fields and variants
   additively; never silently rename a variant or field (wire-stability tests
   guard this); bump the protocol version on any breaking change.
5. **One source of truth.** The daemon (`yerdd`) owns runtime state. The CLI and
   the GUI are both `yerd-ipc` *clients* — neither reimplements daemon logic.

## Dependency direction (never violate)

Internal dependencies flow strictly downhill, no cycles:

```
yerd-core ◄── everything
yerd-core ◄── yerd-ipc ◄── yerd-config, yerd-doctor, binaries, gui
yerd-tls  ◄── yerd-platform ◄── yerd-php, yerd-proxy, binaries
```

- `yerd-core` depends on no other `yerd-*` crate.
- Libraries never depend on binaries.
- The CLI and GUI depend on `yerd-ipc` (+ its `transport` feature), not on the
  daemon's internals.

## Hard rules (enforced or required)

- **No `unsafe`.** `unsafe_code` is `forbid` workspace-wide; crate roots also
  carry `#![forbid(unsafe_code)]`.
- **No `unwrap` / `expect` / `panic!` / `todo!` / `dbg!` / indexing-slicing** in
  non-test code. These are clippy `deny` lints. In tests, allow them explicitly
  at the top of the test file.
- **Errors:** `thiserror` typed errors in libraries; `anyhow` only at binary top
  level. Never add `anyhow` to a library's runtime dependency graph.
- **TLS is rustls + rcgen. Never OpenSSL / native-tls.** Several crates have a
  dep-graph test that fails if an OpenSSL variant leaks in.
- **Async only at the I/O edge.** Pure crates/modules are sync and runtime-free;
  only I/O layers touch `tokio`.
- **`yerd-helper` is the security boundary.** It is the only privileged surface:
  strict typed args, never shell out, never take network input, do exactly one
  operation, then exit. The GUI process must **never** run as root.
- **Document public items.** `missing_docs` is `warn`; pedantic clippy is on.
- **Pin dependencies in `[workspace.dependencies]`** and reference them with
  `dep.workspace = true`. Some versions are pinned with `=` for MSRV or
  wire-stability reasons; do not bump a pinned crate without understanding the
  comment next to it in the root `Cargo.toml`.

## Cross-platform discipline

- Per-OS code is selected with `#[cfg(target_os = ...)]`; exactly one of
  `linux` / `macos` / `unsupported` is active per build. When you touch one OS
  path, make the equivalent change (or a deliberate, commented no-op) in the
  others — a change that compiles only on the host OS will break CI on the other.
- Keep OS-specific *decisions* in pure helper functions (e.g. parsing
  `profiles.ini`, planning ports, matching PEM) so they are unit-testable
  without the OS effect.
- Watch for latent bugs that only surface on one OS (e.g. macOS-only test
  fixtures or path assumptions). CI runs on Linux and macOS.

## Definition of done

A change is complete when:

- pure logic has table-driven unit tests;
- every side-effecting path is behind a trait and tested with a fake;
- wiring has an integration test in the crate's `tests/`;
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` all pass (Linux + macOS);
- frontend changes pass `npm run test` and `npm run build` in `apps/yerd-gui`;
- public items are documented and no `unwrap`/`expect`/`panic` exists outside tests.

## When a task conflicts with these boundaries

Stop and surface the conflict rather than working around it. In particular: if a
task asks you to add I/O to a pure crate, route a side effect around a trait,
give the GUI privileged access, or break the IPC contract — flag it instead of
implementing it.
