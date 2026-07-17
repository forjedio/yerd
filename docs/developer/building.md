# Building from Source

This page is the contributor reference for building, testing, and linting Yerd
from a fresh checkout. Every command, toolchain pin, and CI step below is taken
directly from the repository - [`rust-toolchain.toml`](https://github.com/forjedio/yerd/blob/main/rust-toolchain.toml),
the workspace [`Cargo.toml`](https://github.com/forjedio/yerd/blob/main/Cargo.toml),
[`.github/workflows/ci.yml`](https://github.com/forjedio/yerd/blob/main/.github/workflows/ci.yml),
and the GUI's [`apps/yerd-gui/README.md`](https://github.com/forjedio/yerd/blob/main/apps/yerd-gui/README.md).

For the user-facing "drop binaries on your `PATH`" recipe, see the
[Getting Started](../guide/getting-started) guide. This page goes deeper: the
*why* behind the toolchain, the exact gate CI enforces, and the dependency pins
you must not bump blindly.

## The toolchain

Yerd uses a two-tier toolchain story, and it is important to understand both
tiers before touching a manifest.

**The build/dev toolchain is pinned in [`rust-toolchain.toml`](https://github.com/forjedio/yerd/blob/main/rust-toolchain.toml):**

```toml
[toolchain]
channel = "1.96.0"
components = ["clippy", "rustfmt"]
```

When you run any `cargo` command inside the repo, `rustup` reads this file and
installs / selects `1.96.0` (with `clippy` and `rustfmt` available)
automatically. You do not need to choose a toolchain manually - that is the
whole point of the pin. CI verifies the active toolchain with
`rustup show active-toolchain` before doing anything else.

**Why 1.96 and not the claimed MSRV?** The workspace [`Cargo.toml`](https://github.com/forjedio/yerd/blob/main/Cargo.toml)
declares `rust-version = "1.77"` for the pure library crates, and that MSRV is
real - the library crates (`yerd-core`, `yerd-config`, `yerd-tls`, …) genuinely
compile on 1.77. But the workspace also contains the Tauri v2 GUI crate
(`apps/yerd-gui/src-tauri`). Current Tauri v2 (`tauri-utils`, its plugins) pulls
`toml 1.x` / `serde_spanned 1.x`, which require **edition2024**, which in turn
requires **rustc ≥ 1.85**. The GUI simply cannot build on 1.77. Because
`cargo build --workspace` and the CI gate compile *everything* (including the
GUI crate), the build toolchain has to be new enough for the GUI - hence the
1.96 pin.

::: info Two numbers, two meanings
- **1.77** - the MSRV advertised in the library crate manifests. It is the floor
  the *pure* crates promise to keep working on. Do not raise it casually; it is a
  compatibility commitment.
- **1.96** - the toolchain you actually build and test with. It is bumped only
  when something (like Tauri) forces it.

Edition is `2021` for the workspace package; the edition2024 requirement comes
only from transitive GUI dependencies, not from Yerd's own code.
:::

## Prerequisites

### Rust

Install Rust via [`rustup`](https://rustup.rs). On first `cargo` invocation in
the repo, `rustup` honours `rust-toolchain.toml` and pulls `1.96.0` plus
`clippy` and `rustfmt`. Nothing else to configure.

### Linux system `-dev` packages (for the GUI crate)

The GUI crate links GTK / WebKit / the system tray, so building or even
`clippy`/`test`-ing the *whole workspace* on Linux needs the Tauri `-dev`
headers. The runtime libraries alone are not enough - you need the development
headers. This is exactly what CI installs on its Ubuntu runner:

```sh
sudo apt-get install -y --no-install-recommends \
  libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev \
  libdbus-1-dev libxdo-dev librsvg2-dev
```

The GUI README additionally lists `build-essential` and `pkg-config` for a clean
host. `libdbus-1-dev` is needed by `tauri-plugin-single-instance` and the
appindicator tray; `libxdo-dev` by the tray input layer.

::: tip Library-only builds skip all of this
If you only care about the CLI and daemon, you don't need GTK/WebKit at all -
just build the binaries you want (see [Building only the binaries](#building-only-the-binaries-no-gui)).
On macOS the GUI uses system frameworks, so there are **no** extra packages to
install for the full workspace.
:::

### Node 22 + npm (for the frontend and docs)

The desktop app's frontend and this documentation site are built with Node. CI
uses **Node 22** with `npm`. Install Node 22 (any version manager - `nvm`,
`fnm`, `volta` - works; the GUI README notes the dev host uses `fnm`).

## Building

Build the entire workspace - all library crates, all binaries, and the GUI Rust
bridge - with:

```sh
cargo build --workspace
```

For optimised binaries, add `--release`. Release builds strip symbols
(`[profile.release] strip = "symbols"` in the workspace manifest) to keep the
packaged `.deb` small; a debug `yerdd` is ~139 MB. The trade-off is that shipped
panic backtraces lose symbol names - acceptable given the project's
no-panic/no-unwrap rule (see [Lints and conventions](#lints-and-conventions)).

### Building only the binaries (no GUI)

If you don't want to install the GTK/WebKit toolchain, build just the three
binaries. This is the from-source path from the [README](https://github.com/forjedio/yerd/blob/main/README.md):

```sh
cargo build --release -p yerd -p yerdd -p yerd-helper
install -Dm755 target/release/{yerd,yerdd,yerd-helper} -t ~/.local/bin
yerdd serve &                # rootless; runs on 8080/8443 out of the box
```

The three binaries map to the three-process privilege model: the
[`yerdd`](./binaries/yerdd) daemon, the [`yerd`](./binaries/yerd) CLI, and the
privileged one-shot [`yerd-helper`](./binaries/yerd-helper). See
[Elevation & Privileges](../guide/elevation) for why they are separate.

## Running a from-source build with a production Yerd installed

Most contributors keep the released Yerd app installed for day-to-day work and
want to test a from-source build without losing that setup. The catch: the
installed app runs the daemon as a **per-user service**, and the daemon is a
*singleton* - it takes an exclusive instance lock on its runtime directory and
owns the IPC socket the GUI and CLI connect to. Start a second `yerdd` against
the same runtime dir and it exits immediately:

```text
another yerdd is already running (lock held at /tmp/yerd-501/yerd.lock)
```

So there are two workflows: **take over** the production paths (stop production,
run your build in its place) or **isolate** your build onto a separate set of
paths (Linux only). Pick based on your platform and what you're testing.

### Step 1 - stop the production daemon and GUI

First quit the desktop app from the tray / menu-bar icon - it's a daemon
*client*, so quitting it doesn't stop the daemon, but it stops the app from
issuing requests while you work.

Then stop the daemon service itself. The service label is `dev.yerd.daemon` on
macOS (a GUI-scoped `LaunchAgent`) and the `yerd` systemd **user** unit on Linux:

```sh
# macOS
launchctl kill SIGTERM "gui/$(id -u)/dev.yerd.daemon"

# Linux
systemctl --user stop yerd
```

Confirm nothing is listening before you continue - `yerd ping` should now fail:

```sh
yerd ping   # expect a connection error once the daemon is down
```

### Step 2 - rebuild

Rebuild whatever you changed. For daemon/CLI work the binaries are enough (no
GTK/WebKit needed):

```sh
cargo build -p yerdd -p yerd
```

### Step 3 - run your build in the foreground

Run the daemon you just built directly from the workspace. `serve` is the
default subcommand, and `-v` turns up logging so you can watch it come up:

```sh
cargo run -p yerdd -- -v
```

On a rootless host it binds the fallback ports (`http=8080`, `https=8443`) and
prints the socket, DNS, and mail-capture bindings as it starts. Drive it with
your freshly built CLI in another shell:

```sh
cargo run -p yerd -- ping
cargo run -p yerd -- status
```

To exercise the desktop app against your dev daemon, start the daemon first,
then run the GUI in dev mode (see [The frontend](#the-frontend-apps-yerd-gui)):

```sh
cd apps/yerd-gui && npm run tauri dev
```

### Pointing at a different config file

`yerdd` takes `--config` (`-c`) to override **just** the `yerd.toml` location,
which is handy for testing config changes without touching your real one:

```sh
cargo run -p yerdd -- --config ~/yerd-dev.toml -v
```

::: warning `--config` does not isolate the instance
`--config` swaps only the config *file*. The data, state, cache, and - crucially
- the **runtime socket** still resolve to the normal platform directories, so a
daemon started this way still collides with the instance lock above. Use it
*after* stopping the production daemon, not alongside it. For a fully parallel
instance, isolate the directories instead (next section, Linux only).
:::

### Fully isolating a parallel instance (Linux)

On Linux the directory layout comes from the XDG base-directory variables, so
you can point an entire dev instance at a scratch tree - separate config, data,
state, cache, **and** socket - and run it *alongside* the production daemon
without stopping anything:

```sh
export XDG_CONFIG_HOME=/tmp/yerd-dev/config
export XDG_DATA_HOME=/tmp/yerd-dev/data
export XDG_STATE_HOME=/tmp/yerd-dev/state
export XDG_CACHE_HOME=/tmp/yerd-dev/cache
export XDG_RUNTIME_DIR=/tmp/yerd-dev/run   # different socket → no lock clash
cargo run -p yerdd -- -v
```

Any `yerd` CLI (or `npm run tauri dev` GUI) launched with the *same* environment
resolves the same scratch socket and talks to your dev daemon; a shell without
those variables still reaches production. Tear the instance down by deleting
`/tmp/yerd-dev`.

::: warning macOS has no equivalent override
On macOS the config/data/state/cache paths are fixed to
`~/Library/Application Support/io.yerd.Yerd` and the socket to `/tmp/yerd-$UID`,
with **no** environment override (the daemon and GUI must agree on a single,
discoverable socket path). A from-source daemon therefore shares production's
directories and socket. On macOS you must **stop the production daemon first**
(Step 1) and accept that your build reads and writes the same state - you can't
run two isolated instances side by side.
:::

### Step 4 - restore production

When you're done, stop your foreground daemon (`Ctrl-C`), then bring the service
back and relaunch the app:

```sh
# macOS
launchctl kickstart -k "gui/$(id -u)/dev.yerd.daemon"

# Linux
systemctl --user start yerd
```

## Running tests

```sh
cargo test --workspace
```

This runs every crate's unit and integration tests. The test suite is large and
fast by design: pure logic lives in the library crates and is exercised against
in-memory fakes, while real filesystem / network / process / OS calls sit behind
traits (`ProcessSpawner`, `TrustStore`, `ResolverInstaller`, `PortBinder`,
`Clock`, …) with one implementation per OS. This is what lets the same behaviour
tests run identically on macOS and Linux. The [Cross-Platform Model](./cross-platform)
page covers the trait boundary in detail.

::: warning macOS-only latent test bugs
The trait-fake design means most tests pass on either OS regardless of which
host you're on - which can *hide* a bug that only the real per-OS implementation
would surface. When you change OS-specific code under `yerd-platform`, run the
suite on the affected platform, not just your own. CI runs the full gate on both
`ubuntu-22.04` and `macos-14` (Apple Silicon) for exactly this reason.
:::

## The CI gate (run this before pushing)

CI enforces one gate, and it is identical to what you should run locally. From
[`.github/workflows/ci.yml`](https://github.com/forjedio/yerd/blob/main/.github/workflows/ci.yml),
the `rust` job runs these three commands on both Linux and macOS:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run all three before opening a PR - they are the exact bar your PR must clear.

| Step | Command | Notes |
|---|---|---|
| Format | `cargo fmt --all --check` | CI runs this on Linux only, but formatting is platform-independent, so checking locally on either OS is sufficient. |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | `--all-targets` covers tests/benches/examples too. **Any** warning fails the build (`-D warnings`). |
| Test | `cargo test --workspace` | Full workspace, both OSes in CI. |

::: tip One-liner
```sh
cargo fmt --all --check && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace
```
:::

A second CI job, `frontend`, runs the GUI's JS tests and production build (see
below). The `concurrency` config cancels older in-progress runs when you push
again to the same ref, so only your latest push is graded.

## The frontend (`apps/yerd-gui`)

The desktop app's frontend is **Vue 3 (`<script setup>`) + TypeScript +
Tailwind**, bundled by Vite, tested with Vitest, and type-checked with `vue-tsc`.
All commands run from `apps/yerd-gui`. CI's `frontend` job mirrors this:

```sh
npm ci         # reproducible install from package-lock.json (CI uses this, not `npm install`)
npm run test   # vitest run - frontend unit/component tests
npm run build  # vue-tsc --noEmit && vite build - type-check + production bundle
```

::: info `npm ci` vs `npm install`
CI uses `npm ci` for a clean, lockfile-exact, reproducible install. For local
day-to-day work the GUI README uses `npm install` (or `npm run dev` for the Vite
dev server). Both are fine locally; CI requires `npm ci`.
:::

The available `package.json` scripts:

| Script | Command | Purpose |
|---|---|---|
| `npm run dev` | `vite` | Vite dev server (frontend only, no Rust bridge). |
| `npm run tauri dev` | `tauri` | Full app: webview + Rust bridge. Start `cargo run -p yerdd` first - the GUI is a daemon client. |
| `npm run build` | `vue-tsc --noEmit && vite build` | Type-check, then the production Vite build. |
| `npm run test` | `vitest run` | Run the frontend test suite once. |
| `npm run test:watch` | `vitest` | Vitest in watch mode. |
| `npm run typecheck` | `vue-tsc --noEmit` | Type-check only. |
| `npm run preview` | `vite preview` | Preview a built bundle. |

The Rust side of the bridge is part of the workspace, so its unit tests run under
`cargo test --workspace` (or in isolation via `cargo test -p yerd-gui`, which
needs the Linux `-dev` packages). The bridge is deliberately thin: each Tauri
command maps to a single IPC `Request` to the daemon. See
[Desktop App Internals](./gui) and the [IPC Protocol](./ipc-protocol) page.

## The `=` dependency pins (do not bump blindly)

The workspace manifest contains several **exact** version pins (`=x.y.z`) plus a
long comment block explaining each one. They exist for two distinct reasons -
**MSRV protection** and **wire-stability tripwires** - and bumping one without
understanding its category can break fresh checkouts or silently weaken a safety
net. Always read the comment in [`Cargo.toml`](https://github.com/forjedio/yerd/blob/main/Cargo.toml)
before touching a pin.

### MSRV-driven pins

Several transitive dependencies have introduced edition2024 requirements in newer
releases. Even though the build toolchain is 1.96, these pins keep the *resolved*
dependency graph buildable and the library MSRV story honest. The manifest pins
`time = "=0.3.36"` (newer pulls `time-core 0.1.8` → edition2024) and `clap`,
`tempfile`, etc. to specific versions. Some pins live in `Cargo.lock` rather than
the manifest (the lockfile is the source of truth), applied via
`cargo update -p <crate> --precise <ver>`:

| Crate | Pinned to | Why |
|---|---|---|
| `time` | `=0.3.36` (manifest) | `0.3.37+` pulls `time-core 0.1.8`, which needs edition2024. |
| `indexmap` | `2.13.0` (lockfile) | `2.14+` requires edition2024. |
| `idna_adapter` | `1.0.0` (lockfile) | `1.2+` needs rustc 1.86; pulled transitively via `hickory-proto`'s `idna`. Without it, fresh checkouts fail to even parse the manifest under older cargo. |
| `jobserver` | `0.1.32` (lockfile) | `0.1.34+` pulls a `getrandom 0.3 → wasi 0.14 → wit-bindgen` chain whose manifest needs edition2024. Comes in via `cc → ring → rustls/rcgen`. |
| `hyper-rustls` | `0.27.5` (lockfile) | `0.27.6+` needs rustc 1.85. Pulled by `reqwest`, only via `yerd-php`'s optional `download` feature - invisible to the default build, only bites `--all-features`. |

These all relax once the MSRV moves past 1.85 / edition2024.

### Wire-stability tripwire pins

Two pins are **not** about MSRV - they convert silent upstream additions to
`#[non_exhaustive]` error/data enums into a deliberate, reviewed version bump:

| Crate | Pinned to | Why |
|---|---|---|
| `rcgen` | `=0.13.2` | `rcgen::Error` is `#[non_exhaustive]`; the pin flips the `rcgen_error_detail_table_is_current` tripwire test in [`yerd-tls`](./crates/yerd-tls) if upstream adds a variant. |
| `hickory-proto` / `hickory-server` / `hickory-client` | `=0.24.4` | Same reasoning for `ProtoErrorKind` / `RData`, which are `#[non_exhaustive]` upstream - used by [`yerd-dns`](./crates/yerd-dns). |

::: warning Before bumping any `=` pin
1. Read the comment block at the bottom of [`Cargo.toml`](https://github.com/forjedio/yerd/blob/main/Cargo.toml) - it documents every pin's reason.
2. If it is a **tripwire** pin (`rcgen`, `hickory-*`), expect the bump to trip a test (e.g. `rcgen_error_detail_table_is_current`). Update the corresponding mapping table in the affected crate *deliberately*, don't just silence the test.
3. If it is an **MSRV** pin, confirm the new version still builds the workspace and doesn't drag in a fresh edition2024 dependency.
4. Run the full gate (`fmt` + `clippy` + `test`) on **both** OSes - many of these traps only surface on a clean lockfile resolution.
:::

## Lints and conventions

The workspace declares strict lints in [`Cargo.toml`](https://github.com/forjedio/yerd/blob/main/Cargo.toml)
that the `clippy -D warnings` gate enforces:

```toml
[workspace.lints.rust]
unsafe_code  = "forbid"
missing_docs = "warn"

[workspace.lints.clippy]
unwrap_used      = "deny"
expect_used      = "deny"
panic            = "deny"
todo             = "deny"
dbg_macro        = "deny"
indexing_slicing = "deny"
pedantic         = { level = "warn", priority = -1 }
```

In practice: no `unsafe`, no `unwrap`/`expect`/`panic` outside tests, no
`todo!`/`dbg!`, no slice indexing that could panic - all clippy-enforced. Use
`thiserror` in libraries and `anyhow` only at binary top level. The
[Contributing](./contributing) guide expands on these conventions.

## Packaging and releasing

Build automation lives in the [`xtask`](./xtask) crate, invoked as
`cargo xtask <command>`. It exposes two subcommands:

```sh
cargo xtask bump 2.0.2           # set the version across the three manifests
cargo xtask version-check v2.0.2 # release gate: assert a tag matches the manifests
```

The shipped artifacts are the **GUI bundle** (`.dmg` on macOS, `.deb` on Linux),
**plus a native Arch package** (`.pkg.tar.zst`, x86-64) **and a Fedora package**
(`.rpm`, x86-64 and arm64). The three binaries
(`yerd`/`yerdd`/`yerd-helper`) are embedded via Tauri `externalBin`
(per-platform overlays in `apps/yerd-gui/src-tauri/`). Tauri builds the `.app`
(macOS) and `.deb` (Linux) directly; the macOS `.dmg` is built as a separate
headless step (`apps/yerd-gui/scripts/build-macos-dmg.sh`, via `appdmg`) after
the `.app`, not by Tauri's own dmg bundler - see
[macOS code signing & notarisation](#macos-code-signing-notarisation) below
for why. The CLI and daemon are never shipped on their own - there is no
CLI-only artifact (tarball or `.deb`) separate from the GUI bundle and the
Arch package.

`bump` keeps three files in sync - `Cargo.toml`,
`apps/yerd-gui/src-tauri/tauri.conf.json`, and `apps/yerd-gui/package.json` - so
the CLI/daemon and the GUI never disagree on version. The release pipeline runs
`version-check` to fail fast on a mismatched tag. See
[Build Automation (xtask)](./xtask) for the full breakdown.

### The Arch package (`.pkg.tar.zst`)

Tauri has no pacman bundler, so the Arch package is built separately: the `arch`
job in [`release.yml`](https://github.com/forjedio/yerd/blob/main/.github/workflows/release.yml)
runs an `archlinux:base-devel` container, compiles the frontend + all four
binaries from source, and assembles the package with
[`packaging/arch/PKGBUILD`](https://github.com/forjedio/yerd/blob/main/packaging/arch).
The package installs the four binaries as real files in `/usr/bin` - the three
driven binaries (`yerd`/`yerdd`/`yerd-helper`) land at the same paths as the
upstream `.deb`, so the daemon's sibling-binary lookup is identical (the GUI binary
is `/usr/bin/yerd-gui`). A `.install` scriptlet `setcap`s `/usr/bin/yerdd` on
install/upgrade so the daemon can bind ports 80/443.

In-app `yerd update` on Arch runs `pacman -U` on the downloaded, minisign-verified
`.pkg.tar.zst` - a **partial upgrade**: if the host is behind on `pacman -Syu`, a
newer library soname can make it abort (Yerd surfaces pacman's message), so Arch
users should keep their system current. It also requires the default
`LocalFileSigLevel = Optional` in `pacman.conf` - the package is not pacman-signed
(Yerd verifies it itself with the embedded update key), so a hardened
`LocalFileSigLevel = Required` rejects the local install.

The Arch package's minisign signature is published as `<pkg>.pkg.tar.zst.minisig`,
not `.sig`. pacman reserves `<pkg>.sig` for a detached OpenPGP signature and feeds
any such sibling to GPGME, so a minisign file under that name hard-fails
`pacman -U` even at `LocalFileSigLevel = Optional` (a missing signature is
tolerated, a present-but-unparseable one is fatal) - this was
[#157](https://github.com/forjedio/yerd/issues/157). The other artifact kinds
(`.app.tar.gz`, `.deb`, `.rpm`) publish a byte-identical legacy `<artifact>.sig`
copy alongside the `.minisig` for a bounded transition window, so self-updaters
built at v2.0.3 or earlier keep resolving a signature; that copy is retired once
those clients have moved on, and the pacman `.sig` is never re-published (the
release workflow fails the publish if a `*.pkg.tar.zst.sig` is present).

### The Fedora package (`.rpm`)

Unlike pacman, Tauri v2 **does** have an rpm bundler (pure-Rust, no `rpmbuild`),
so the `.rpm` reuses the normal Tauri bundle machinery rather than a hand-written
spec. The `fedora` jobs in
[`build.yml`](https://github.com/forjedio/yerd/blob/main/.github/workflows/build.yml)
run on the Ubuntu runners (x86-64 and arm64), build the sidecars with
`--features yerdd/rpm`, and run `tauri build` with the
`tauri.bundle-linux-rpm.conf.json` overlay (rpm `depends` + an `rpm/postinst.sh`
`%post` that `setcap`s `/usr/bin/yerdd`, mirroring the deb postinst). Because the
bundler runs on Ubuntu, a **blocking** `fedora:latest` smoke job in `release.yml`
`dnf install`s the produced `.rpm` (proving every `Requires` resolves) and asserts
`yerdd --pkg-format` before the release can publish.

In-app `yerd update` on Fedora runs `rpm -U --oldpackage` on the downloaded,
minisign-verified `.rpm`. Like `dpkg -i`/`pacman -U` it does no dependency
*resolution*, but it does *check* dependencies, so the packaged `depends` list must
stay stable across releases (a newly-added `Requires` would abort an existing
user's self-update); Yerd surfaces rpm's message on failure.

**The `pacman`/`rpm` feature / `PkgFormat` tiebreak.** A release carries a `.deb`,
a `.pkg.tar.zst`, and a `.rpm` for the same arch, so a running Linux binary can't
tell which to self-update from `Platform` (OS + arch) alone. The format is fixed at
**build time**: `yerd_update::PkgFormat::current()` returns `Pacman` only when
compiled with the `pacman` Cargo feature and `Rpm` only with the `rpm` feature
(the two are mutually exclusive - enabling both is a `compile_error!`). The `arch`
job builds the daemon with `--features yerdd/pacman`, the `fedora` job with
`--features yerdd/rpm`, so each `yerdd` selects its own package and the applier
installs it via `pacman -U` / `rpm -U --oldpackage`; every other build defaults to
`Deb`/`dpkg -i`. **This only works because each distro package is built in a
separate job (its own cargo invocation/target dir)** - within one cargo build, the
feature would unify across the whole graph. The release gate proves the flag took by
running the freshly-built `yerdd --pkg-format` and asserting it prints
`pacman`/`rpm`.

**`tauri/custom-protocol` is required too.** `PKGBUILD`'s `build()` builds the
GUI binary with a raw `cargo build`, not `tauri build`/`npm run tauri build` -
so it must pass `--features yerdd/pacman,tauri/custom-protocol` explicitly.
`tauri build` injects `custom-protocol` automatically; a bare `cargo build`
doesn't. Without it, Tauri's `generate_context!()` bakes `dev: true` into the
binary, and at runtime it tries to load `devUrl` (`http://localhost:1420`)
instead of the embedded frontend `dist` - the GUI fails to load at all, with
"Could not connect to localhost: Connection refused".

### macOS code signing & notarisation

The release workflow Developer ID signs **and** notarises the macOS artifact:
the GUI `.app` (signed, notarised and stapled by Tauri) and its `.dmg`
(codesigned only, by `apps/yerd-gui/scripts/build-macos-dmg.sh` after Tauri
builds the `.app` - not notarised or stapled, and deliberately so: only the
`.app` staple is enforced in CI, the `.dmg`'s own notarisation/staple is
advisory and non-fatal, since the stapled `.app` inside is the gate). The
three embedded binaries (`yerd`/`yerdd`/`yerd-helper`) are signed by Tauri **as
part of the bundle** (Hardened Runtime + secure timestamp + the app's Developer ID
team) and covered by the single `.app` notarisation - so there are no loose,
separately-notarised CLI binaries. Notarisation uses an **App Store Connect API
key**. The CI verify step asserts each embedded binary is Developer-ID signed,
Hardened-Runtime, timestamped, team-matched, and free of broad entitlements
(`allow-jit` / `allow-unsigned-executable-memory` / `get-task-allow`).

This is driven entirely by GitHub Actions **secrets** - there's nothing to
configure in a normal build. To (re)provision them:

| Secret | What it is |
|---|---|
| `APPLE_CERTIFICATE` | base64 of the exported **Developer ID Application** `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)` |
| `APPLE_API_ISSUER` | App Store Connect API **Issuer ID** |
| `APPLE_API_KEY` | App Store Connect API **Key ID** (not the file) |
| `APPLE_API_KEY_P8` | base64 of the `AuthKey_<KEYID>.p8` key file |

**Rotation.** Developer ID certificates last ~5 years - regenerate from a new CSR
(Keychain Access → Certificate Assistant), export a fresh `.p12`, and update
`APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD`/`APPLE_SIGNING_IDENTITY`. API
keys are revocable in App Store Connect → Users and Access → Integrations; create
a replacement (role **Developer**) and update `APPLE_API_*`/`APPLE_API_KEY_P8`.

The GUI's signing config lives in `apps/yerd-gui/src-tauri/tauri.conf.json`
(`bundle.macOS`) and `apps/yerd-gui/src-tauri/entitlements.plist` (the
Hardened-Runtime entitlements - note it must **not** carry `get-task-allow`).

**Verifying a release.** The `gui` job verifies fail-closed before publishing. To
check by hand on a Mac: `xcrun stapler validate Yerd.app` (should pass - this is
the actual gate), `spctl -a -t open --context context:primary-signature -vvv
Yerd.dmg` (expect `source=Unnotarized Developer ID` - the `.dmg` itself is
signed-only, not notarised, so this is normal and not a sign of a broken
release), and `codesign -dv --verbose=4 Yerd.app/Contents/MacOS/yerdd` (expect
`Authority=Developer ID Application`).

## See also

- [Architecture](./architecture) - how the pieces fit together.
- [Crates Overview](./crates) - the workspace's crate map and boundaries.
- [Cross-Platform Model](./cross-platform) - the per-OS trait implementations.
- [Contributing](./contributing) - workflow and conventions.
