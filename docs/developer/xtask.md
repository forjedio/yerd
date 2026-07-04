# Build Automation (xtask)

Yerd's repository-local build automation lives in the [`xtask`](https://github.com/forjedio/yerd/tree/main/xtask) crate. It follows the well-known [cargo-xtask](https://github.com/matklad/cargo-xtask) pattern: instead of shell scripts or a Makefile, build tasks are ordinary Rust programs run through a cargo alias. There is no extra tool to install - if you can build the workspace, you can run every task.

```sh
cargo xtask <command>
```

The alias is declared in the repository's `.cargo/config.toml`:

```toml
[alias]
# Run build-automation tasks: `cargo xtask <cmd>` (e.g. `cargo xtask bump`).
xtask = "run --package xtask --"
```

So `cargo xtask bump` expands to `cargo run --package xtask -- bump`. The crate is a normal workspace member (listed in the root `Cargo.toml` `members` array) and inherits the workspace version, edition, MSRV, license, and lint configuration. Its only dependencies are `clap` (argument parsing) and `anyhow` (error handling). That small surface is deliberate.

## What it does today

`xtask` exposes exactly two subcommands. They are declared as a `clap` enum in `xtask/src/main.rs`:

| Command | Purpose |
|---|---|
| `cargo xtask bump <version>` | Set the project version across all three manifests. |
| `cargo xtask version-check <version>` | Assert a tag/version matches all three manifests (release gate). |

```rust
/// `xtask` subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
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
`xtask` only keeps **versions in sync**. It does *not* build any artifacts - the shipped products are the **GUI bundle** (`.dmg` on macOS, `.deb` on Linux) with the three binaries (`yerd`/`yerdd`/`yerd-helper`) embedded via `externalBin` (per-platform overlays in `apps/yerd-gui/src-tauri/`), plus a native **Arch package** (`.pkg.tar.zst`, x86-64) built from `packaging/arch/PKGBUILD` in the workflow's `arch` job. Tauri builds the `.app` (macOS) and `.deb` (Linux) directly; the macOS `.dmg` is packaged as a separate headless step (`apps/yerd-gui/scripts/build-macos-dmg.sh`, via `appdmg`) since Tauri's own dmg bundler drives Finder via AppleScript, which isn't reliable outside an interactive session. There is no standalone CLI tarball/`.deb`. `xtask` also doesn't download or cache PHP - PHP builds are fetched at runtime by the daemon (see [yerd-php](./crates/yerd-php)). Cross-platform release artifacts and checksums are assembled by the GitHub Actions workflows.
:::

## Module map

The crate is split into a **pure helper** (deterministic, I/O-free, unit-tested) and **thin orchestration** (the glue that reads/writes files and prints).

```
xtask/
├── Cargo.toml
└── src/
    ├── main.rs      orchestration: CLI parse + bump/version-check I/O glue
    └── version.rs   PURE: in-place version edits/reads + sync assertion
```

This mirrors the project-wide convention: **pure logic in functions you can test in-memory; I/O pushed to the edges.** `version.rs` carries the logic with no `std::fs` calls; `main.rs` does the reading and writing.

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

The version commands exist to make tagged releases safe, but cutting a release
is a single command: [`scripts/release.sh`](https://github.com/forjedio/yerd/blob/main/scripts/release.sh).

```sh
./scripts/release.sh --version v2.0.3           # final release
./scripts/release.sh --version 2.0.2-rc.5       # prerelease (leading v optional)
./scripts/release.sh --version v2.0.3 --dry-run # print the plan, change nothing
./scripts/release.sh --version v2.0.3 --tag-only # push the tag only, not the branch
```

It normalises the version (strips a leading `v`) and validates it against
`MAJOR.MINOR.PATCH[-prerelease]` - the exact shape the release workflow's tag
filter accepts (`v2.0.2.rc-5` is rejected; use `v2.0.2-rc.5`). Before touching
anything it checks the tree is clean (`git diff` / `git diff --cached`), that
you're on a real branch (not detached `HEAD`), and that the tag doesn't already
exist locally or on the remote. It then:

1. Runs `cargo xtask bump <version>` to set `Cargo.toml`, `tauri.conf.json`,
   and `package.json`.
2. Runs `cargo update --workspace` to refresh `Cargo.lock` to match (a failure
   here, e.g. offline, is a warning, not fatal).
3. Runs `cargo xtask version-check <tag>` - the same gate CI runs - to confirm
   every manifest actually landed on the new version.
4. Commits the bump as `Release: vX.Y.Z`, tags it (`git tag -a`), and pushes -
   the branch and the tag by default, or just the tag with `--tag-only`.

The release CI builds the single GUI bundle for macOS (`arm64`) and Linux (`amd64` and `arm64`) - each embedding the three binaries - and a mismatched tag fails fast via `cargo xtask version-check`. Because `version-check` strips the leading `v`, it accepts both the tag form (`v2.0.2`) and the bare version (`2.0.2`). See [Building from Source](./building) and [Contributing](./contributing) for the full developer gate.

## Conventions and invariants

- **Pure / orchestration split.** Version parsing/editing and the sync assertion live in `version.rs` and are unit-tested in-memory; `main.rs` only does I/O. This matches the [architecture](./architecture) principle used across the codebase.
- **No `unsafe`.** `main.rs` declares `#![forbid(unsafe_code)]`.
- **`anyhow` at the top level.** As a binary, `xtask` uses `anyhow::Result` with `.with_context(...)` for actionable messages, rather than the `thiserror` enums libraries use.

## Source

- Crate root: [`xtask/`](https://github.com/forjedio/yerd/tree/main/xtask)
- Orchestration: [`xtask/src/main.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/main.rs)
- Pure helper: [`xtask/src/version.rs`](https://github.com/forjedio/yerd/blob/main/xtask/src/version.rs)
