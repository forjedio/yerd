# Contributing to Yerd

Thanks for your interest in contributing! This guide covers how to get set up,
the conventions we follow, and what "done" looks like.

This is the quick-start summary. The **canonical, in-depth contributor
reference** lives in the developer docs - read these for the full detail:

- [Building from Source](docs/developer/building.md) - toolchain, prerequisites,
  the CI gate, the dependency pins, packaging.
- [Contributing](docs/developer/contributing.md) - layering, error handling, the
  testing standard, wire-stability, and the Definition of Done.

> **AI agents:** the authoritative, always-on rules live in
> [`.github/copilot-instructions.md`](.github/copilot-instructions.md) and the
> path-specific `.github/instructions/*.instructions.md` files. This document and
> the developer docs are the human-friendly summaries; where they overlap they
> agree - if they ever drift, the instructions files win.

## What Yerd is

Yerd is a rootless, cross-platform local PHP development environment. It serves
projects on `.test` domains over HTTP/HTTPS, runs multiple PHP versions per
site, and supervises databases and caches as native child processes - no Docker,
no `sudo` for everyday work.

It's a **Rust workspace** plus a **Tauri v2 + Vue 3** desktop app. macOS and
Linux are supported today; Windows is planned but its OS adapters don't exist
yet - don't assume Windows code paths.

## Getting set up

You'll need:

- **Rust** - the toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml)
  (currently 1.96.0); `rustup` will pick it up automatically. Note that the pure
  library crates still target a 1.77 MSRV, but the GUI requires the newer
  toolchain to build.
- **Node 22 + npm** - for the desktop app frontend under `apps/yerd-gui` and the
  docs site. (CI uses Node 22; any version manager - `nvm`, `fnm`, `volta` -
  works.)
- **Linux only:** the GTK/WebKit/tray `-dev` packages the Tauri GUI crate links
  against, plus [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/).
  The exact `apt` list is in [Building from Source](docs/developer/building.md).
  macOS needs no extra packages. If you only build the CLI and daemon, you can
  skip all of this.

Clone and build:

```bash
git clone https://github.com/forjedio/yerd.git
cd yerd
cargo build --workspace
```

Run the CLI or daemon directly from the workspace:

```bash
cargo run -p yerd -- --help
cargo run -p yerdd             # the daemon (serve is the default subcommand)
```

> Already have the released app installed? See
> [Running a from-source build with a production Yerd installed](docs/developer/building.md#running-a-from-source-build-with-a-production-yerd-installed)
> for how to stop the production daemon, isolate a dev instance, and restore it.

Work on the desktop app:

```bash
cd apps/yerd-gui
npm install
npm run tauri dev
```

## Architecture in one rule

> **Pure logic lives in library crates. I/O and OS calls are pushed to the edges
> behind traits.**

Everything else follows from that:

- **Pure crates/modules do no I/O** - no filesystem, network, process spawning,
  clock, or env reads. They're sync, runtime-free, and unit-testable with
  in-memory fixtures. `yerd-core` is the exemplar.
- **Side effects go behind traits** (`ProcessSpawner`, `TrustStore`,
  `PortBinder`, `Clock`, …). Logic depends on the trait; tests inject a fake; the
  real impl lives in `yerd-platform` or a crate's `os` module behind `#[cfg(...)]`.
- **Binaries are thin.** `bin/yerdd`, `bin/yerd`, `bin/yerd-helper`, and the
  Tauri `src-tauri` layer wire crates together; they hold orchestration, not
  behaviour.
- **One source of truth.** The daemon (`yerdd`) owns runtime state. The CLI and
  GUI are both `yerd-ipc` clients - neither reimplements daemon logic.
- **The IPC protocol is a stable contract.** Add fields and variants additively;
  never rename a variant/field (wire-stability tests guard this); bump the
  protocol version on a breaking change.

Internal dependencies flow strictly downhill (no cycles): `yerd-core` depends on
no other `yerd-*` crate, and libraries never depend on binaries.

## Hard rules

These are enforced by lints, dependency-graph tests, or review - please don't
work around them:

- **No `unsafe`.** It's `forbid` workspace-wide.
- **No `unwrap` / `expect` / `panic!` / `todo!` / `dbg!` / indexing-slicing** in
  non-test code (clippy `deny`). In tests, allow them explicitly at the top of
  the file.
- **Errors:** `thiserror` typed errors in libraries; `anyhow` only at binary top
  level - never in a library's runtime dependency graph.
- **TLS is rustls + rcgen.** Never OpenSSL / native-tls; a dep-graph test
  enforces this.
- **Async only at the I/O edge.** Pure crates/modules stay sync and runtime-free.
- **`yerd-helper` is the security boundary** - the only privileged surface.
  Strict typed args, never shell out, never take network input, do one operation
  then exit. The GUI process must **never** run as root.
- **Pin dependencies** in `[workspace.dependencies]` and reference them with
  `dep.workspace = true`. Some are pinned with `=` for MSRV or wire-stability
  reasons - read the comment before bumping.
- **Document public items** (`missing_docs` is `warn`; pedantic clippy is on).

## Cross-platform discipline

Per-OS code is selected with `#[cfg(target_os = ...)]`. When you touch one OS
path, make the equivalent change (or a deliberate, commented no-op) in the
others - a change that compiles only on the host OS will break CI on the other.
Keep OS-specific *decisions* in pure helper functions so they stay unit-testable
without the OS effect. CI runs on both Linux and macOS.

## Definition of done

A change is complete when:

- pure logic has table-driven unit tests;
- every side-effecting path is behind a trait and tested with a fake;
- wiring has an integration test in the crate's `tests/`;
- the following all pass on **both Linux and macOS**:

  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- frontend changes pass `npm run test` and `npm run build` in `apps/yerd-gui`;
- public items are documented, and no `unwrap`/`expect`/`panic` exists outside
  tests.

## Pull requests

- Branch off `main` and open a PR against it. Fill in the
  [PR template](.github/pull_request_template.md).
- Keep PRs focused; describe the motivation, not just the change.
- Link the issue you're addressing (e.g. `Closes #44`).
- If a task seems to require breaking one of the boundaries above (adding I/O to
  a pure crate, giving the GUI privileged access, breaking the IPC contract),
  **stop and raise it in the issue or PR** rather than working around it.

## Reporting bugs and requesting features

Use the [issue templates](https://github.com/forjedio/yerd/issues/new/choose).
For anything security-sensitive, follow [`SECURITY.md`](SECURITY.md) instead of
opening a public issue.

## License

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE.md).
