# Yerd

A cross-platform local PHP development environment for **macOS, Linux, and Windows**. Yerd serves projects on `.test` domains over HTTP and HTTPS, manages multiple PHP versions per site, and optionally runs MySQL / MariaDB / PostgreSQL / Redis as supervised native processes.

Think Laravel Herd — but cross-platform, open-source, rootless in normal operation, and built on a single statically-typed Rust core that drives both a CLI and a native-feeling GUI.

> **Status:** Early development. The two foundation crates (`yerd-core`, `yerd-ipc`) are in place. Most of the system is not built yet — see [What's next](#whats-next).

> **Lineage note.** This is a ground-up Rust replacement for the author's prior Go project of the same name (`LumoSolutions/yerd`, v1). v1 is reference-only — there is no command-surface compatibility, no config-format compatibility, and no carried-over assumptions (v1 builds PHP from source and elevates with `sudo` for most operations; v2 ships prebuilt PHP and runs unprivileged).

---

## Goals

- **Cross-platform.** macOS, Linux, Windows — one codebase, native installers per OS.
- **Rootless.** Setup may elevate once (install a local CA, configure DNS, grant port-bind capability). Day-to-day operation runs as the user.
- **Per-site PHP version.** Ship prebuilt PHP binaries; let the user pick the version per site.
- **HTTPS by default.** Local CA + per-site leaf certificates, no `mkcert` dance.
- **One source of truth.** A single daemon owns runtime state. The CLI and GUI are both *clients* of the daemon — neither reimplements its logic.
- **Native-feeling UI.** Tauri v2 (system webview) for the desktop app, not Electron — typical installer ~8–15 MB rather than ~100 MB.
- **Optional service supervision.** MySQL, MariaDB, PostgreSQL, Redis — run as Yerd-supervised child processes, no Docker.

---

## Architecture

The single organising rule:

> **Pure logic lives in library crates. I/O and OS calls are pushed to the edges behind traits.**

Concretely:

1. **Pure crates do no I/O** — no filesystem, no network, no process spawning, no clock reads, no environment reads. They are unit-testable with in-memory fixtures and zero setup.
2. **Side effects go behind traits** (`ProcessSpawner`, `TrustStore`, `ResolverInstaller`, `PortBinder`, `Downloader`, `Clock`, …). Business logic depends on the trait; tests inject a fake; one real impl per OS lives in `yerd-platform` or a crate's `os` module behind `#[cfg(...)]`.
3. **Binaries are thin.** The daemon, CLI, and privileged helper binaries wire crates together. Behaviour belongs in crates with tests.
4. **The IPC protocol is a stable contract.** Wire shapes are versioned. Additions are backward-compatible; renames break CI via tag-stability tests.
5. **One source of truth.** The daemon owns state. CLI and GUI are clients.

### Process model

Two long-lived processes plus a one-shot privileged helper:

- **`yerdd`** — the daemon. Runs as the user, owns runtime state, exposes an IPC socket.
- **`yerd-gui`** — the Tauri desktop app. **Never** runs as root.
- **`yerd`** — the CLI. IPC client.
- **`yerd-helper`** — a strict, auditable one-shot binary that performs the few operations that need elevation (install the CA into system trust stores, set `cap_net_bind_service`, install the DNS resolver). Validates every argument, never shells out, never accepts network input, does exactly one thing, exits.

### Stack (locked decisions)

| Concern | Choice |
|---|---|
| Core language | Rust (edition 2021, MSRV 1.77) |
| GUI | Tauri v2 + Vue 3 (`<script setup>`) + TypeScript + Tailwind |
| TLS | `rustls` (never OpenSSL); `rcgen` for the local CA |
| Reverse proxy | `hyper` + `hyper-util` + `tokio-rustls`, hand-rolled (~600 LOC) |
| DNS | `hickory-dns` embedded resolver answering `*.test` |
| PHP binaries | `static-php-cli` builds per platform/arch/version |
| PHP execution | PHP-FPM per version for MVP; FrankenPHP worker mode later |
| Async runtime | `tokio`, only in I/O layers — never in pure crates |
| IPC transport | Unix domain socket (macOS/Linux) + named pipe (Windows), via `interprocess` |

---

## Repository layout

```
yerd/
├── Cargo.toml                  # workspace manifest
├── rust-toolchain.toml         # pinned stable toolchain (1.77 + rustfmt, clippy, llvm-tools-preview)
├── crates/                     # libraries — pure where possible
│   ├── yerd-core/              # ✅ domain model + host→site routing
│   ├── yerd-ipc/               # ✅ UI/CLI ⇄ daemon protocol + framing
│   ├── yerd-config/            # ✅ persisted config (TOML)
│   ├── yerd-tls/               # 🚧 local CA + per-site leaf certs
│   ├── yerd-dns/               # 🚧 *.test resolver
│   ├── yerd-proxy/             # 🚧 hyper + rustls reverse proxy
│   ├── yerd-php/               # 🚧 PHP-FPM pool supervision + version mgmt
│   ├── yerd-services/          # 🚧 MySQL / MariaDB / Postgres / Redis lifecycle
│   └── yerd-platform/          # 🚧 OS adapters behind traits (trust store, resolver, port binding, autostart, paths, elevation)
├── bin/                        # 🚧 binary targets
│   ├── yerdd/                  # the daemon (orchestration + IPC server)
│   ├── yerd/                   # the CLI (IPC client)
│   └── yerd-helper/            # privileged one-shot
├── apps/                       # 🚧 GUI
│   └── yerd-gui/               # Tauri v2 app: src-tauri (Rust) + Vue frontend
└── xtask/                      # 🚧 build automation
```

Legend: ✅ shipped · 🚧 planned

### Crates

#### `yerd-core` — domain model & routing  · **STATUS: shipped**

The pure heart of Yerd. Defines:

- `PhpVersion` — strict major.minor with a custom serde impl that round-trips as the canonical string `"8.3"`.
- `Tld` — validated DNS suffix newtype (ASCII, lowercased, DNS-label rules).
- `Site` / `SiteKind` — a routable target with a private `name` invariant; renaming is a router-level operation, not a setter.
- `RouterConfig` — typed TLD plus a precomputed `.{tld}` suffix for the resolver hot path.
- `SiteRouter` — `BTreeMap`-backed registry with `new` / `from_sites` / `insert` / `remove` / `get` / `get_mut` / `iter` / `len` / `is_empty` / `config` and the host→site `resolve` algorithm.
- `CoreError` — single error type, every public error enum `#[non_exhaustive]`.

The `resolve` algorithm honours: port stripping, FQDN trailing-dot, case-insensitivity, TLD enforcement, exact-match beats wildcard, and wildcard-subdomain → parent (Valet behaviour). IPv6 literals and non-ASCII hosts are positively rejected.

100% pure: no I/O, no async, no internal `yerd-*` deps. Only `serde` + `thiserror` in `[dependencies]`.

**Test coverage: 96.70% lines** across 79 tests (73 unit + 6 integration), measured with `cargo-llvm-cov`.

#### `yerd-ipc` — protocol & framing  · **STATUS: shipped**

The wire contract between clients (CLI, GUI) and the daemon. Defines:

- `Request` / `Response` / `ErrorCode` — internally tagged JSON (`#[serde(tag = "type", rename_all = "snake_case")]`), every public enum `#[non_exhaustive]` for additive evolution.
- `encode_frame` / `FrameDecoder` / `DEFAULT_MAX_FRAME` (16 MiB) — pure length-prefixed frame codec (4-byte BE `u32` length prefix). Decoder handles partial reads, pipelined frames, and poisoning on oversized declared lengths.
- `encode_message` / `decode_message` — thin `serde_json` wrappers.
- `FrameError` (pure, `Clone + Eq`) + `IpcError` + `IpcErrorKind` (`Clone + Eq` shadow for Tauri/GUI consumers that can't clone `serde_json::Error`).
- `PROTOCOL_VERSION` — exposed for future use; a `Hello`/`Welcome` handshake will land before the first breaking change.
- `types` module re-exporting `yerd_core::{Site, PhpVersion, SiteKind}` so downstream consumers can depend on `yerd-ipc` alone.
- Optional `transport` feature (gated on `tokio`): `write_message`, `read_frame`, `read_message` generic over `AsyncRead`/`AsyncWrite`. Socket and named-pipe binding stays in the binaries.

Default build is pure (no `tokio`, no async, no I/O, no `tracing`). Tag-stability tests pin every wire shape; inline `variant_name_pinning` modules catch Rust-side variant renames at compile time. A grep gate forbids per-field `#[serde(rename = "...")]` so the rename trap is symmetrical (Rust name == JSON tag, enforced).

**Test coverage: 57 tests** (21 inline unit + 16 frame-codec + 7 round-trip + 13 wire-stability) with `--features transport`. Six dependencies: `yerd-core`, `serde`, `serde_json`, `thiserror`, and (optional) `tokio`.

#### `yerd-config` — persisted configuration  · **STATUS: shipped**

The on-disk TOML schema and round-trip pipeline. Defines:

- `Config` — top-level schema with private `version` plus `tld`, `ports`, `php`, `parked`, `linked`, `services`. `Default` produces a `version = CURRENT_VERSION` config that round-trips cleanly.
- `Ports` (`well_known()` = 80/443, `unprivileged()` = 8080/8443), `PhpSection` (`default = 8.3`), `ParkedSection` (`BTreeSet<String>` paths), `ServicesSection` (`BTreeSet<String>` enabled, validated against `KNOWN_SERVICES`).
- `Config::from_toml` / `to_toml` / `validate` — pure parse/serialise/validate. Wire mirrors are raw-typed (`String`, `PathBuf`) so `yerd-core` per-field validation failures surface as typed `ConfigError::Core(CoreError)` rather than collapsing into `serde::de::Error::custom`. Every nested wire mirror carries `#[serde(deny_unknown_fields)]`.
- `Config::load` / `save` — thin I/O leaves; `save` uses `tempfile::NamedTempFile` + `persist` for write-temp-then-rename (atomic via `rename(2)` on Unix, `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows).
- `ConfigError` — `Parse`, `Serialize`, `Validate { reason }`, `Core`, `UnsupportedVersion`, `Migration { reason }`, `Io { path: PathBuf, source }`. Every public error enum `#[non_exhaustive]`; reason sub-enums each carry an explicit `Display` impl.
- `CURRENT_VERSION` + the `migrate::{STEPS, MigrationStep, read_version, up}` scaffold. Zero migrations in v0; v1→v2 lands without parse-path restructuring.

Public schema types deliberately do **not** derive `Serialize` / `Deserialize` — the crate-internal wire mirrors handle the TOML round-trip, keeping the public surface free of an accidental serde contract.

**Test coverage: 91.61% lines** across 72 tests (54 unit + 18 integration). Workspace lints green; `cargo fmt`/`clippy --workspace -D warnings`/`test`/`llvm-cov --fail-under-lines 80` all pass.

#### `yerd-tls` — local CA & leaf certificates  · **STATUS: planned**

mkcert-equivalent CA + per-site leaf issuance via `rcgen`. Pure crypto: callers pass PEM strings; no disk, no trust-store install, no TLS server.

#### `yerd-dns` — `.test` resolver  · **STATUS: planned**

Pure responder ("given a query name + TLD, return the answer") + a `hickory-dns` server that calls it. Does **not** configure the OS resolver — that's `yerd-platform::ResolverInstaller`.

#### `yerd-proxy` — reverse proxy  · **STATUS: planned**

Hand-rolled `hyper` + `rustls` reverse proxy. Listens on 80/443 (or 8080/8443 rootless), selects the leaf cert per SNI, and forwards to PHP-FPM (FastCGI on Unix sockets / TCP on Windows) or a FrankenPHP worker. WebSocket and HTTP/2 pass-through.

#### `yerd-php` — PHP-FPM supervision  · **STATUS: planned**

Per-version FPM pool config, spawn/health-check/restart state machine, version discovery (Yerd's bundled `static-php-cli` builds plus optional `mise` integration). Process spawning behind a `ProcessSpawner` trait; downloads behind a `Downloader` trait.

#### `yerd-services` — databases & caches  · **STATUS: planned**

DBngin-style lifecycle for MySQL, MariaDB, PostgreSQL, Redis as native child processes (no Docker). Generic supervisor driven by `ServiceDefinition` descriptors. Downloads SHA-256-verified.

#### `yerd-platform` — OS abstraction layer  · **STATUS: planned**

Per-OS, often-privileged operations behind traits: `Paths`, `TrustStore`, `ResolverInstaller`, `PortBinder`, `Autostart`, `Elevation`. One thin implementation per OS selected by `#[cfg(...)]`.

---

## What's been built

- **Workspace scaffolding.** `Cargo.toml`, `rust-toolchain.toml` pinned to stable 1.77 with `rustfmt`, `clippy`, `llvm-tools-preview`. Lint table lifted to `[workspace.lints]` so every crate inherits the same `unsafe_code = "forbid"` + clippy `unwrap`/`expect`/`panic`/`indexing_slicing`/`pedantic` posture.
- **`yerd-core` v0.1.0.** Complete — 7 modules, 9 public types, 79 tests, 96.70% line coverage, zero `unwrap`/`expect`/`panic`/indexing in non-test code.
- **`yerd-ipc` v0.1.0.** Complete — 7 modules, 57 tests with `--features transport`, length-prefixed JSON framing with poisoning on oversized frames, internally-tagged enums with byte-exact wire pins, async transport helpers gated behind an opt-in feature. Default build is pure (no `tokio`, no I/O). See the [crate-level README](crates/yerd-ipc/README.md) for the wire-stability policy and the no-rename rule.
- **`yerd-config` v0.1.0.** Complete — 7 modules, 72 tests, 91.61% line coverage; raw-typed wire mirrors keep `yerd-core` validation failures surfaceable as typed `ConfigError::Core`; schema-versioned with a forward-migration scaffold; atomic `save` via `tempfile::NamedTempFile::persist`. See the [crate-level README](crates/yerd-config/README.md) for I/O semantics, path-storage rationale, and the TLD-trailing-dot normalisation note.
- **Cross-crate wire-stability gates.** `crates/yerd-core/tests/wire_stability.rs` pins the JSON byte shape of every `yerd-core` type that travels over IPC; `crates/yerd-ipc/tests/wire_stability.rs` pins every `Request`, `Response`, and `ErrorCode` variant. A rename anywhere fails CI before any client sees a divergent format.

### Local gate (run from the repo root)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo llvm-cov --package yerd-core --fail-under-lines 80
! grep -REn '#\[serde\([^)]*[^_[:alnum:]]?rename[[:space:]]*=' crates/yerd-ipc/src/
```

All gates are currently green on Linux. `--all-features` exercises the `yerd-ipc` `transport` feature and its async smoke tests. `cargo-llvm-cov` is an out-of-band tool (`cargo install cargo-llvm-cov --version 0.6.15 --locked`); the rest is part of the standard toolchain.

---

## What's next

Order matters — each crate is built against the contracts of the one beneath it. The "must-not" rules from each crate (no internal deps upwards, no I/O in pure crates) prevent cycles by construction.

**Phase 0 — foundations.**
1. `yerd-core` ✅
2. `yerd-ipc` ✅

**Phase 1 — MVP (macOS + Linux first).**
3. `yerd-config` ✅
4. `yerd-tls` — CA + leaf issuance (pure crypto).
5. `yerd-platform` — `Paths`, then `TrustStore`, `ResolverInstaller`, `PortBinder` (macOS + Linux impls first).
6. `yerd-dns` — `.test` responder + hickory server.
7. `bin/yerd-helper` — `install-ca`, `install-resolver`, `setcap`.
8. `yerd-php` — FPM config render + supervision (one bundled version to start).
9. `yerd-proxy` — HTTP first, then HTTPS via `yerd-tls` cert store.
10. `bin/yerdd` — wire 1–9 together; IPC server transport.
11. `bin/yerd` — `park` / `link` / `sites` / `use` / `secure` against the daemon.

**Phase 2 — v1.**
12. `apps/yerd-gui` — tray-first Tauri UI over IPC.
13. `yerd-services` — MySQL/MariaDB/Postgres/Redis.
14. `yerd-platform` Windows impls + `yerd-php` TCP-loopback backend for Windows.
15. `xtask` — `static-php-cli` build matrix, bundling, signing, auto-updater wiring.

**Later.** FrankenPHP worker mode, deeper `mise` integration, mail catcher, dump debugger, Xdebug auto-toggle, tunnelling.

---

## Conventions

- Edition 2021, MSRV stable 1.77.
- `thiserror` in libraries; `anyhow` only at binary top level.
- No `unwrap` / `expect` / `panic!` outside `#[cfg(test)]` (clippy-enforced).
- `unsafe_code = "forbid"` on every crate.
- `tracing` for logs in everything that does I/O; pure crates emit nothing.
- Pure crates are synchronous and runtime-free. Only I/O layers touch `tokio`.
- Routing rules and other behavioural contracts are pinned by table-driven tests — new behaviour stays table-driven.

---

## License

MIT OR Apache-2.0 (per workspace package metadata).
