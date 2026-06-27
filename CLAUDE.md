# CLAUDE.md

Guidance for Claude Code (and other AI agents) working in this repository. This
file is a **general overview**; the detailed, authoritative rules live in the
`.github` instruction files — follow the pointers below rather than guessing.

## Source of truth for the rules

- **[`.github/copilot-instructions.md`](.github/copilot-instructions.md)** — the
  always-on baseline: architecture, hard rules, cross-platform discipline, and
  the definition of done. **Read this first.**
- **[`.github/instructions/*.instructions.md`](.github/instructions/)** —
  path-scoped rules that apply automatically to the files they match (each has an
  `applyTo` glob). Before editing a crate or binary, read its file — e.g.
  `yerd-core.instructions.md`, `yerdd.instructions.md`,
  `yerd-helper.instructions.md`, `yerd-gui-frontend.instructions.md`. The
  workspace-wide `rust-all-crates.instructions.md` covers all `**/*.rs`.
- **[`CONTRIBUTING.md`](CONTRIBUTING.md)** and the developer docs
  ([`docs/developer/`](docs/developer/)) — the human-facing versions of the same
  material (build/test, layering, testing standard, packaging).

These agree with one another by design. If anything here ever conflicts with the
`.github` instruction files, **the instruction files win.**

## What Yerd is

A fast, rootless, open-source **local PHP development environment** for macOS and
Linux (Windows is planned; its OS adapters don't exist yet — don't assume Windows
code paths). It serves projects on `.test` domains over HTTP/HTTPS, runs a
different PHP version per site, and supervises databases, caches, mail capture,
and dumps — no Docker, no `sudo` for everyday work.

It ships as a **single desktop app**: a Tauri v2 + Vue 3 GUI, the `yerd` CLI, and
a privileged one-shot helper, all thin clients over a small background daemon
(`yerdd`) that owns all runtime state.

## Architecture in one rule

> **Pure logic lives in library crates. I/O and OS calls are pushed to the edges
> behind traits.**

Consequences (see `copilot-instructions.md` for the full list):

- **Pure crates/modules do no I/O** — no filesystem, network, process spawning,
  clock, or env reads; sync and runtime-free; unit-testable with in-memory
  fakes. `yerd-core` is the exemplar.
- **Side effects go behind traits** (`ProcessSpawner`, `TrustStore`, `PortBinder`,
  `Clock`, …); real impls live in `yerd-platform` or a crate's `os/` module
  behind `#[cfg(...)]`.
- **Binaries are thin** (`bin/yerdd`, `bin/yerd`, `bin/yerd-helper`, the Tauri
  `src-tauri` layer) — orchestration, not behaviour.
- **One source of truth:** the daemon owns runtime state; the CLI and GUI are
  both `yerd-ipc` clients and never reimplement daemon logic.
- **The IPC protocol is a stable contract** — evolve it additively; wire-stability
  tests guard byte shapes.

Dependency direction flows strictly downhill (no cycles): `yerd-core` depends on
no other `yerd-*` crate; libraries never depend on binaries.

## Repository layout

| Path | What it is |
|---|---|
| `crates/` | Library crates (`yerd-core`, `yerd-ipc`, `yerd-config`, `yerd-tls`, `yerd-platform`, `yerd-dns`, `yerd-php`, `yerd-proxy`, `yerd-doctor`, …) |
| `bin/` | The three binaries: `yerdd` (daemon), `yerd` (CLI), `yerd-helper` (privileged one-shot) |
| `apps/yerd-gui/` | Tauri v2 desktop app — `src-tauri/` (Rust bridge) + `src/` (Vue 3 + TS + Tailwind) |
| `xtask/` | Build/release automation, run as `cargo xtask <cmd>` |
| `docs/` | VitePress documentation site (published at yerd.app) |
| `.github/` | CI workflows, issue/PR templates, and the agent instruction files |

## Hard rules (enforced — don't work around)

- **No `unsafe`** (`forbid` workspace-wide).
- **No `unwrap` / `expect` / `panic!` / `todo!` / `dbg!` / indexing-slicing** in
  non-test code (clippy `deny`). Tests may opt out explicitly at the top of the
  file.
- **Errors:** `thiserror` typed errors in libraries; `anyhow` only at binary top
  level — never in a library's dependency graph.
- **TLS is rustls + rcgen.** Never OpenSSL / native-tls.
- **Async only at the I/O edge** — pure crates/modules stay sync.
- **`yerd-helper` is the security boundary** — the only privileged surface; the
  GUI must never run as root.
- **Pin dependencies** in `[workspace.dependencies]`; read the comment beside any
  `=`-pin before bumping it.
- **Mirror per-OS changes** across `linux` / `macos` / `unsupported`; CI runs on
  both Linux and macOS.

## Build, test, and the CI gate

The toolchain is pinned in `rust-toolchain.toml` (1.96.0; the pure library crates
keep a 1.77 MSRV). Run the full gate — identical to CI — before considering a
change done, on **both Linux and macOS** where possible:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Frontend changes additionally run, from `apps/yerd-gui`:

```sh
npm run test    # vitest
npm run build   # vue-tsc --noEmit && vite build
```

Run the CLI or daemon from source with `cargo run -p yerd` / `cargo run -p yerdd`.
For build prerequisites, isolating a dev instance against an installed
production Yerd, and packaging, see
[`docs/developer/building.md`](docs/developer/building.md).

## Definition of done

A change is complete when pure logic has table-driven unit tests, every
side-effecting path is behind a trait and tested with a fake, wiring has an
integration test, public items are documented, and the full gate passes on both
OSes. The complete checklist is in
[`docs/developer/contributing.md`](docs/developer/contributing.md).

## When a task conflicts with these boundaries

Stop and surface the conflict rather than working around it — in particular if a
task would add I/O to a pure crate, route a side effect around a trait, give the
GUI privileged access, or break the IPC contract. Flag it instead of
implementing it.

## Working agreement

- **Commits are the user's job.** Don't commit, push, or merge without being
  asked — make the edits and leave them staged for review.
- Match the surrounding code's style, naming, and comment density.
