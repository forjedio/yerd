# Build Automation (xtask)

Yerd's repository-local build automation lives in the [`xtask`](https://github.com/forjedio/yerd/tree/main/xtask) crate. It follows the well-known [cargo-xtask](https://github.com/matklad/cargo-xtask) pattern: instead of shell scripts or a Makefile, build tasks are ordinary Rust programs run through a cargo alias. There is no extra tool to install - if you can build the workspace, you can run every task.

```sh
cargo xtask <command>
```

The alias is declared in the repository's `.cargo/config.toml`:

```toml
[alias]
# Run build-automation tasks: `cargo xtask <cmd>` (e.g. `cargo xtask deb`).
xtask = "run --package xtask --"
```

So `cargo xtask deb` expands to `cargo run --package xtask -- deb`. The crate is a normal workspace member (listed in the root `Cargo.toml` `members` array) and inherits the workspace version, edition, MSRV, license, and lint configuration:

```toml
[package]
name                   = "xtask"
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
publish.workspace      = true
description            = "Build automation for Yerd (cargo xtask <cmd>)."

[dependencies]
anyhow = { workspace = true }
clap   = { workspace = true }
flate2 = { workspace = true }
```

Its only dependencies are `clap` (argument parsing), `anyhow` (error handling), and `flate2` (gzip for the Debian changelog). That small surface is deliberate.

## What it does today

`xtask` exposes exactly three subcommands. They are declared as a `clap` enum in `xtask/src/main.rs`:

| Command | Purpose |
|---|---|
| `cargo xtask deb` | Build a Linux `.deb` package for the three Yerd binaries. |
| `cargo xtask bump <version>` | Set the project version across all three manifests. |
| `cargo xtask version-check <version>` | Assert a tag/version matches all three manifests (release gate). |

```rust
/// `xtask` subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Build a Linux `.deb` package for the Yerd binaries.
    Deb(deb::DebArgs),
    /// Set the project version across Cargo.toml, tauri.conf.json, package.json.
    Bump {
        /// The new version, e.g. `2.0.2` or `2.0.2-rc.1` (a leading `v` is fine).
        version: String,
    },
    /// Assert the given tag/version matches all three manifests (release gate).
    VersionCheck {
        /// The tag/version to check, e.g. `v2.0.2` (a leading `v` is stripped).
        version: String,
    },
}
```

::: info Scope
`xtask` builds the **Debian package** and keeps **versions in sync**. It does *not* build the GUI bundles (`.dmg` / `.AppImage` / GUI `.deb`) - those are produced by Tauri in the release pipeline - and it does *not* download or cache PHP. PHP builds are fetched at runtime by the daemon (see [yerd-php](./crates/yerd-php)); `xtask` deliberately bundles no PHP. Cross-platform release artifacts and checksums are assembled by the GitHub Actions workflows, not by `xtask`.
:::

## Module map

The crate is intentionally split into **pure helpers** (deterministic, I/O-free, heavily unit-tested) and **thin orchestration** (the glue that touches the filesystem, spawns processes, and prints).

```
xtask/
├── Cargo.toml
├── src/
│   ├── main.rs      orchestration: CLI parse + bump/version-check I/O glue
│   ├── deb.rs       orchestration: stage the package tree, shell out to dpkg-deb
│   ├── pack.rs      PURE: control-file rendering, arch mapping, filename, version parse
│   └── version.rs   PURE: in-place version edits/reads + sync assertion
└── assets/          static files embedded into the .deb at compile time
    ├── postinst
    ├── yerd.service
    ├── copyright
    └── changelog.Debian
```

This mirrors the project-wide convention used throughout the Yerd crates: **pure logic in functions you can test in-memory; I/O pushed to the edges.** In `xtask`, `pack.rs` and `version.rs` carry the logic and have no `std::fs`/`std::process` calls; `main.rs` and `deb.rs` do the reading, writing, and process spawning.

## `cargo xtask deb` - Debian packaging

`deb.rs` stages a Debian package tree on disk and then invokes the system `dpkg-deb` to build it. PHP is **not** bundled - the package ships only the three binaries, a systemd **user** unit, and metadata.

### Arguments

```rust
/// Arguments to `cargo xtask deb`.
#[derive(clap::Args, Debug)]
pub struct DebArgs {
    /// Target Debian architecture (defaults to the host arch).
    #[arg(long)]
    pub arch: Option<String>,
    /// Output directory for the staged tree and the `.deb` (default
    /// `target/debian`).
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// Skip the release build and package the existing `target/release` binaries.
    #[arg(long)]
    pub no_build: bool,
}
```

| Flag | Default | Effect |
|---|---|---|
| `--arch <arch>` | host arch (`std::env::consts::ARCH`) | Target Debian architecture. Mapped through `pack::debian_arch`. |
| `--out-dir <dir>` | `target/debian` | Where the staged tree and the resulting `.deb` are written. |
| `--no-build` | off | Skip the `cargo build --release` step and package whatever is already in `target/release`. |

### What `run` does, step by step

1. **Tool check.** `ensure_tool("dpkg-deb", …)` runs `dpkg-deb --version` and bails with a remediation hint (`install it with: sudo apt install dpkg-dev`) if it is missing.
2. **Resolve the architecture.** The host arch (or `--arch`) is mapped to a Debian arch via `pack::debian_arch`. Unsupported arches fail with a clear error.
3. **Build (optional).** Unless `--no-build` is set, it runs `cargo build --release -p yerd -p yerdd -p yerd-helper`. The cargo binary honours the `CARGO` environment variable if set, falling back to `cargo`.
4. **Verify the binaries exist.** Each of `yerd`, `yerdd`, `yerd-helper` must be present in `target/release`, otherwise it bails (telling you to run a release build or drop `--no-build`).
5. **Read the version.** It runs `target/release/yerd --version` and extracts the version token with `pack::parse_version` - the package version is taken from the *binary itself*, not re-read from a manifest.
6. **Wipe stale staging.** Any prior `yerd_<version>_<arch>` staging directory is removed so a re-run never ships leftover files.
7. **Stage the tree** (see below).
8. **Build the package.** It shells out to `dpkg-deb --build --root-owner-group <stage> <deb_path>`. `--root-owner-group` forces `root:root` ownership in the archive regardless of who runs the build.
9. **Print next steps**, including how to install and enable the daemon.

The three binaries are fixed in a constant, and the maintainer string is hard-coded:

```rust
/// Maintainer recorded in the package `control` file.
const MAINTAINER: &str = "Forjed <support@forjed.io>";
/// The three binaries shipped in `usr/bin`.
const BINARIES: [&str; 3] = ["yerd", "yerdd", "yerd-helper"];
```

### The staged package tree

`stage_tree` lays down a standard binary-package layout:

```
yerd_<version>_<arch>/
├── DEBIAN/
│   ├── control          rendered from pack::render_control(&DebMeta { … })
│   └── postinst         embedded assets/postinst, chmod 0755
└── usr/
    ├── bin/
    │   ├── yerd         chmod 0755
    │   ├── yerdd        chmod 0755
    │   └── yerd-helper  chmod 0755
    ├── lib/systemd/user/
    │   └── yerd.service embedded assets/yerd.service
    └── share/doc/yerd/
        ├── copyright           embedded assets/copyright
        └── changelog.Debian.gz gzip(embedded assets/changelog.Debian)
```

The four static files are embedded into the binary at compile time with `include_str!`, so the tool is self-contained and the assets are tracked in version control:

```rust
// Static package assets, embedded so they ship with the tool.
const POSTINST: &str = include_str!("../assets/postinst");
const SERVICE_UNIT: &str = include_str!("../assets/yerd.service");
const COPYRIGHT: &str = include_str!("../assets/copyright");
const CHANGELOG: &str = include_str!("../assets/changelog.Debian");
```

The `control` file metadata is assembled into a `DebMeta`. Notably the package **depends on `libcap2-bin`** (it provides `setcap`, used by the postinst) and lands in section `devel`, priority `optional`:

```rust
let meta = DebMeta {
    package: "yerd".to_owned(),
    version: version.to_owned(),
    arch: arch.to_owned(),
    maintainer: MAINTAINER.to_owned(),
    section: "devel".to_owned(),
    priority: "optional".to_owned(),
    depends: "libcap2-bin".to_owned(),
    description: /* one-line synopsis + extended description */,
};
```

The changelog is gzipped deterministically - `flate2`'s default gzip header uses mtime 0 - so identical inputs produce byte-identical output.

### The `postinst` and the `cap_net_bind_service` capability

The single most important behaviour of the package is in `assets/postinst`. Yerd's daemon runs **unprivileged**, but to serve on the standard ports it must bind 80/443. The post-install grants `yerdd` the `cap_net_bind_service` file capability so the daemon can do that without ever running as root:

```sh
#!/bin/sh
set -e
case "$1" in
  configure)
    # Let the unprivileged daemon bind 80/443. Reapplied on every upgrade because
    # dpkg replaces the binary, which wipes its file capabilities.
    if command -v setcap >/dev/null 2>&1; then
      setcap 'cap_net_bind_service=+ep' /usr/bin/yerdd \
        || echo "yerd: setcap failed; yerdd will fall back to ports 8080/8443" >&2
    fi
    echo "yerd: enable the daemon with:  systemctl --user enable --now yerd"
    echo "      to keep it running after logout:  loginctl enable-linger \"\$USER\""
    ;;
esac
exit 0
```

::: warning Why it re-applies on upgrade
dpkg's `postinst` runs with `$1 == "configure"` on both **fresh install and upgrade**, and upgrading **replaces the `yerdd` binary**, which wipes its file capabilities. Because the same `configure` branch runs every time, the `setcap` is re-applied on every upgrade - without it, an upgraded daemon would silently lose the ability to bind 80/443. If `setcap` is unavailable or fails, the package does not error: it prints a warning and the daemon falls back to ports 8080/8443 (and `yerd doctor` surfaces this). See [Elevation & Privileges](../guide/elevation).
:::

This invariant is pinned by a unit test in `deb.rs`, so an accidental edit to the asset breaks the build:

```rust
#[test]
fn postinst_reapplies_setcap_under_configure() {
    // The postinst must re-apply this on upgrade (dpkg wipes file caps), so
    // guard the exact line against accidental edits.
    assert!(POSTINST.contains("setcap 'cap_net_bind_service=+ep' /usr/bin/yerdd"));
    assert!(POSTINST.contains("configure)"));
}
```

### The systemd user unit

`assets/yerd.service` is installed to `usr/lib/systemd/user/yerd.service` so the daemon runs as a **per-user** service, never system-wide:

```ini
[Unit]
Description=Yerd local PHP development daemon

[Service]
Type=simple
ExecStart=/usr/bin/yerdd serve
Restart=on-failure

[Install]
WantedBy=default.target
```

A second `deb.rs` test guards the two load-bearing lines (`ExecStart=/usr/bin/yerdd serve` and `WantedBy=default.target`). After install you enable it with `systemctl --user enable --now yerd`, and `loginctl enable-linger "$USER"` keeps it running after logout - the exact strings the `postinst` and `deb` output print. See [The Daemon](../guide/daemon).

## Pure packaging helpers - `pack.rs`

`pack.rs` holds the deterministic, I/O-free pieces of packaging. Each function is small and individually unit-tested.

```rust
/// Render a [`DebMeta`] as a Debian `control` stanza (trailing newline included).
pub fn render_control(meta: &DebMeta) -> String;

/// Map a Rust target arch (std::env::consts::ARCH) to a Debian arch.
/// Returns `None` for arches this packaging path does not support yet.
pub fn debian_arch(rust_arch: &str) -> Option<&'static str>;

/// The conventional `.deb` filename: `{package}_{version}_{arch}.deb`.
pub fn deb_filename(package: &str, version: &str, arch: &str) -> String;

/// Extract a version from a `--version` line (e.g. "yerd 0.1.0" → "0.1.0").
pub fn parse_version(version_output: &str) -> Option<String>;
```

Notable details verified in the source and tests:

- **`render_control`** emits the `Description` field per Debian policy: the first line is the synopsis, and each further line becomes a leading-space continuation line. A blank extended-description line is written as the special ` .` (space-dot) form.
- **`debian_arch`** maps `x86_64 → amd64` and `aarch64 → arm64`, returning `None` for anything else (e.g. `riscv64`) so unsupported targets fail fast rather than producing a mislabelled package.
- **`deb_filename`** produces the canonical `yerd_0.1.0_amd64.deb` shape.
- **`parse_version`** takes the last whitespace-separated token of the first non-empty line and returns `None` for empty/whitespace-only input.

## Version sync - `bump` and `version-check`

Yerd declares its version in **three** manifests that must never drift:

| Manifest | Key |
|---|---|
| `Cargo.toml` (workspace root) | `[workspace.package].version` |
| `apps/yerd-gui/src-tauri/tauri.conf.json` | top-level `"version"` |
| `apps/yerd-gui/package.json` | top-level `"version"` |

`main.rs` locates all three relative to the crate's manifest directory (the workspace root is `xtask`'s parent), and `version.rs` provides the pure string transforms.

### `cargo xtask bump <version>`

```sh
cargo xtask bump 2.0.2        # or 2.0.2-rc.1, or v2.0.2 (the leading v is fine)
```

This sets the version in all three files and prints a reminder to commit and tag. The leading `v` is stripped by `version::normalise`. The edit is **surgical**: it rewrites only the single version line in each file, preserving indentation, the key, any trailing comma, and the original trailing-newline behaviour - it never reformats the whole document. The same `replace_last_string` helper works for both TOML (`version = "X"`) and JSON (`"version": "X",`) because in both cases the value is the *last* double-quoted string on the line.

```rust
/// Set `[workspace.package].version` in a `Cargo.toml`.
pub fn set_cargo(content: &str, version: &str) -> Result<String>;
/// Set the top-level `"version"` in a JSON manifest (tauri.conf.json / package.json).
pub fn set_json(content: &str, version: &str) -> Result<String>;
```

The Cargo helper specifically scans for the `version` key **inside the `[workspace.package]` table**, so a `version = "1"` in `[workspace.dependencies]` is never touched - a behaviour pinned by a unit test.

### `cargo xtask version-check <version>`

This is the **release gate**. It reads the version out of all three manifests and asserts they all equal the (normalised) expected version:

```rust
/// Assert all three found versions equal `expected`. Returns a human-readable
/// error listing every mismatch when they don't.
pub fn assert_all_match(expected: &str, found: &[Found]) -> Result<()>;
```

On success it prints `OK: all manifests are at <version>`. On mismatch it lists *every* offending manifest (omitting the ones that match) and tells you to run `cargo xtask bump <expected>`, commit, and re-tag.

## Release workflow

The version commands exist to make tagged releases safe. The flow (from the project README) is:

```sh
cargo xtask bump 2.0.2          # sets Cargo.toml + tauri.conf.json + package.json
git commit -am "release: v2.0.2" && git tag v2.0.2 && git push --follow-tags
```

The release CI then builds the CLI artifacts (`.deb` + tarballs) for macOS and Linux on both `amd64` and `arm64`, plus GUI bundles for macOS (`arm64`) and Linux (`amd64` only - there is no `arm64` Linux GUI runner), and a mismatched tag fails fast via `cargo xtask version-check`. Because `version-check` strips the leading `v`, it accepts both the tag form (`v2.0.2`) and the bare version (`2.0.2`). See [Building from Source](./building) and [Contributing](./contributing) for the full developer gate.

## Conventions and invariants

- **Pure / orchestration split.** All logic with edge cases (control rendering, arch mapping, version parsing/editing, sync assertion) lives in `pack.rs` and `version.rs` and is unit-tested in-memory; `main.rs` and `deb.rs` only do I/O and process spawning. This matches the [architecture](./architecture) principle used across the codebase.
- **No `unsafe`.** `main.rs` declares `#![forbid(unsafe_code)]`.
- **`anyhow` at the top level.** As a binary, `xtask` uses `anyhow::Result` with `.with_context(...)` for actionable messages, rather than the `thiserror` enums libraries use.
- **Determinism.** Staging wipes any prior tree before building; the gzipped changelog uses a zeroed mtime; ownership is forced to `root:root` via `--root-owner-group`. The same inputs yield the same package.
- **Embedded assets.** Package metadata files are compiled into the binary with `include_str!`, so there is no runtime asset lookup and the package contents are reproducible from source.
- **Fail fast with hints.** Missing `dpkg-deb`, missing release binaries, or an unsupported arch all abort with a message telling you exactly what to do.

## Source

- Crate root: [`xtask/`](https://github.com/forjedio/yerd/tree/main/xtask)
- Orchestration: [`xtask/src/main.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/main.rs), [`xtask/src/deb.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/deb.rs)
- Pure helpers: [`xtask/src/pack.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/pack.rs), [`xtask/src/version.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/version.rs)
- Embedded assets: [`xtask/assets/`](https://github.com/forjedio/yerd/tree/main/xtask/assets)
