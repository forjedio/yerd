<div align="center">

# Yerd

**A fast, rootless, open-source local PHP development environment.**

Serve your projects on `.test` domains over HTTP **and** HTTPS, run a different
PHP version per site, and manage it all from one tiny daemon — no Docker, no
`sudo` for everyday work, no subscription.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Platforms: macOS · Linux](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux-success.svg)](#installation)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Docs](https://img.shields.io/badge/docs-yerd.app-6366f1.svg)](https://yerd.app)
[![Docs deploy](https://github.com/forjedio/yerd/actions/workflows/docs.yml/badge.svg)](https://github.com/forjedio/yerd/actions/workflows/docs.yml)

📖 **[Read the documentation at yerd.app →](https://yerd.app)**

</div>

---

## Why Yerd?

Type a URL like `https://my-app.test` and your site just works, with the right
PHP version and a trusted certificate. Yerd makes local PHP development
frictionless — **cross-platform, fully open-source, and rootless by design.**

- 🚀 **Zero-config sites.** Drop a project in a parked directory and it's instantly
  live at `<name>.test`.
- 🔒 **HTTPS that just works.** A local certificate authority issues per-site
  certificates automatically — no `mkcert` dance, no browser warnings once trusted.
- 🐘 **Any PHP, per site.** Install multiple PHP versions and pin each site to the
  one it needs.
- 🗄️ **Databases & caches, no Docker.** Install and supervise MySQL, MariaDB,
  PostgreSQL, and Redis as native, per-user processes — create, drop, back up, and
  restore databases straight from the CLI.
- 🪶 **Lightweight & native.** A single ~8 MB daemon binary. No containers, no VM,
  no Electron.
- 🛡️ **Rootless.** Setup elevates **once**; everything after runs as your user.
- 🔍 **Self-diagnosing.** `yerd status` and `yerd doctor` tell you exactly what's
  running and how to fix what isn't.

---

## Yerd vs. Herd vs. Lerd

|  | Laravel Herd | Lerd | **Yerd** |
|---|:---:|:---:|:---:|
| Free | ✅ (Pro is paid) | ✅ | ✅ |
| Open source | ❌ | ✅ | ✅ |
| Linux support | ❌ | ✅ | ✅ |
| macOS support | ✅ | ✅ | ✅ |
| Windows support | ✅ | ✅ | ✅ * |
| Automatic `.test` domains | ✅ | ✅ | ✅ |
| HTTPS with a trusted local CA | ✅ | ✅ | ✅ |
| Multiple PHP versions | ✅ | ✅ | ✅ |
| PHP version **per site** | ✅ | ✅ | ✅ |
| First-class CLI | ✅ | ✅ | ✅ |
| Menu-bar / tray GUI | ✅ | ❌ | ✅ |
| Database & cache services (MySQL · MariaDB · PostgreSQL · Redis) | ✅ (Pro) | ✅ | ✅ |
| Runs rootless day-to-day | ✅ | ✅ † | ✅ |
| **No** Docker / Podman / containers required | ✅ | ❌ | ✅ |
| Lightweight (no VM, no container images) | ✅ | ❌ | ✅ |
| Built-in health checks (`doctor`) | ❌ | ❌ | ✅ |
| Under the hood | Native app (nginx + dnsmasq) | Containers (Podman/Docker) | Native Rust (`rustls` proxy + embedded DNS) |

<sub>✅\* = on the Yerd [roadmap](#roadmap). Everything without an asterisk works today on macOS and Linux.</sub>
<br><sub>**Lerd** runs your stack in containers via **Podman/Docker** — so it's
cross-platform and trivially adds database/cache services, but it pulls and runs
container images rather than native processes. † Rootless when run on rootless
Podman.</sub>
<br><sub>**On Laravel Valet:** Valet is the original macOS-only Laravel dev tool
(nginx + dnsmasq, installed via Homebrew/Composer). None of the three require it —
Herd is the native standalone successor that bundles its own nginx (and reuses
Valet's framework "drivers"), Lerd runs everything in containers, and Yerd uses
its own Rust proxy + DNS. No Valet, no Homebrew.</sub>

---

## Installation

> **Yerd runs entirely as your user — never as root.** `sudo` appears in exactly
> two non-ongoing places: installing the system `.deb` (standard for *any*
> package), and a single **one-time** setup step. Day-to-day use needs no
> elevation. Prefer no `sudo` at all? Use the tarball or [build from source](#from-source).

Pre-built artifacts for **macOS and Linux** (x86-64 and arm64) are attached to
each [GitHub Release](https://github.com/forjedio/yerd/releases), each verified
by a `SHA256SUMS` manifest.

### Quick install (CLI + daemon)

```bash
curl -fsSL https://raw.githubusercontent.com/forjedio/yerd/main/scripts/install.sh | sh
```

This fetches the latest release, **verifies it against `SHA256SUMS`**, and
installs `yerd` + `yerdd` + `yerd-helper` — the `.deb` on Debian/Ubuntu
(system-wide), or a tarball to `~/.local/bin` everywhere else (no sudo). On
non-Debian systemd distros (**Arch/Omarchy, Fedora, openSUSE, …**) it also drops
in a `yerd` **user service** so the daemon works the same way. Pin a version with
`YERD_VERSION=2.0.2`.

### Manual download

| Platform | CLI artifact |
|---|---|
| Debian / Ubuntu (amd64 · arm64) | `yerd_<ver>_amd64.deb` · `yerd_<ver>_arm64.deb` → `sudo dpkg -i …` |
| Arch · Fedora · other Linux (rootless) | `yerd-<ver>-{x86_64,aarch64}-unknown-linux-gnu.tar.gz` |
| macOS (Apple Silicon) | `yerd-<ver>-aarch64-apple-darwin.tar.gz` |

Verify against the release's `SHA256SUMS`, then start the per-user daemon:

```bash
sha256sum -c SHA256SUMS --ignore-missing       # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
systemctl --user enable --now yerd             # .deb install (runs as you, not root)
# …or from the tarball:  yerdd serve &          # rootless; 8080/8443 out of the box
```

The `.deb`'s post-install grants `yerdd` `cap_net_bind_service` (so the
**unprivileged** daemon binds 80/443) and re-applies it on every upgrade; if
unavailable, Yerd falls back to `8080`/`8443` (and `yerd doctor` tells you).

### Desktop GUI (optional)

The tray app ships as separate bundles on the same release:

| Platform | GUI artifact | Install |
|---|---|---|
| macOS (Apple Silicon) | `Yerd_<ver>_aarch64.dmg` | open, drag to Applications |
| Linux | `Yerd_<ver>_amd64.AppImage` | `chmod +x` and run |
| Linux | `Yerd_<ver>_amd64.deb` | `sudo dpkg -i …` |

> The GUI is a **client of the daemon** — install the CLI (above) too, so `yerdd`
> is present and the app's privileged "Fix" actions can find `yerd` (on Linux
> both `.deb`s install to `/usr/bin`, which is what the GUI expects).

> **Unsigned for now:** macOS warns on first launch — right-click → **Open**, or
> `xattr -dr com.apple.quarantine /Applications/Yerd.app`.

### One-time setup

Run this **once** for the full experience — the only command that uses root, and
each part is independent:

```bash
sudo yerd elevate            # trust the local CA · route *.test · allow 80/443
# …or pick pieces:  sudo yerd elevate trust | resolver | ports
```

This mirrors the one-time admin step Herd and Valet also need (reconfiguring the
system resolver and trusting a local certificate can't be done rootlessly).
After it, `yerd` never touches root again.

### From source

No system package (or any `sudo` to install)? Build and drop the binaries on
your `PATH` — no root required:

```bash
git clone https://github.com/forjedio/yerd
cd yerd
cargo build --release -p yerd -p yerdd -p yerd-helper
install -Dm755 target/release/{yerd,yerdd,yerd-helper} -t ~/.local/bin
yerdd serve &                # rootless; runs on 8080/8443 out of the box
```

`cargo xtask deb` packages a `.deb` instead. (Browser `*.test` resolution and
trusted HTTPS still need the one-time `sudo yerd elevate`, or drive sites
directly on `127.0.0.1:8080`.)

> PHP itself is **not** bundled — Yerd downloads prebuilt, static PHP builds on
> demand when you run `yerd install php`. Installing Yerd is tiny and fast.

---

## Quick start

```bash
# 1. Install a PHP version and make it the default
yerd install php 8.5
yerd use 8.5

# 2a. Park a directory — every sub-folder becomes <folder>.test
yerd park ~/Sites
#     ~/Sites/blog  ->  http://blog.test

# 2b. …or link a single project under a name you choose
yerd link my-app ~/code/my-app
#     ->  http://my-app.test

# 3. Turn on HTTPS for a site
yerd secure my-app
#     ->  https://my-app.test  (trusted, thanks to the local CA)

# 4. Pin one site to a different PHP version
yerd use my-app 8.3

# 5. See what's going on / fix problems
yerd status
yerd doctor
yerd doctor fix
```

Open `https://my-app.test` in your browser — that's it.

---

## Command reference

| Command | What it does |
|---|---|
| `yerd park <dir>` | Park a directory; each child folder is served at `<name>.test`. |
| `yerd link <name> <dir>` | Serve a single directory as a named site. |
| `yerd unlink <name>` | Remove a linked/parked site. |
| `yerd sites` | List every known site (kind, PHP version, HTTPS, doc-root). |
| `yerd use <version>` | Set the **global** default PHP version. |
| `yerd use <site> <version>` | Set one site's PHP version. |
| `yerd secure <site>` / `unsecure <site>` | Turn HTTPS on / off for a site. |
| `yerd install php <version>` | Download + install a PHP version. |
| `yerd list php [--check]` | List installed PHP versions (and available updates). |
| `yerd update php [<version>]` | Update one (or all) installed PHP versions. |
| `yerd services` | List local database / cache services and their status. |
| `yerd service install <svc> <version>` | Install a service (`redis`/`mysql`/`mariadb`/`postgres`) from a prebuilt build. |
| `yerd service start\|stop\|restart <svc>` | Start, stop, or restart a service (start also enables auto-start). |
| `yerd service set-port <svc> <port>` / `logs <svc>` | Set a service's loopback port; tail its log. |
| `yerd service change-version\|uninstall <svc> …` | Switch a service's version, or remove one (`--purge` deletes its data). |
| `yerd db list\|create\|drop <svc> [<name>]` | List, create, or drop databases in a running SQL service. |
| `yerd db backup\|restore <svc> <name> <file>` | Dump a database to / restore it from a plain-SQL file. |
| `yerd status` | Snapshot: daemon, ports, DNS, CA trust, PHP pools (PID/RAM), load. |
| `yerd doctor` / `yerd doctor fix` | Diagnose common problems; auto-repair the safe ones. |
| `yerd elevate [trust\|resolver\|ports]` | One-time privileged setup (run with `sudo`). |
| `yerd unelevate [...]` | Reverse what `elevate` configured. |

Add `--json` to any command for machine-readable output.

---

## Principles

Yerd is built around a few deliberate decisions that make it safe, fast, and
maintainable.

### 🛡️ Rootless, with a tight privilege boundary

Yerd runs as **three** pieces, and the GUI/daemon **never** run as root:

- **`yerdd`** — the unprivileged per-user daemon. It owns all runtime state and
  serves the proxy, DNS, and PHP-FPM pools.
- **`yerd`** — the CLI, a thin client that just talks to the daemon over a
  per-user socket.
- **`yerd-helper`** — a strict, auditable one-shot binary for the handful of
  operations that genuinely need root (trust the CA, configure the DNS resolver,
  grant the port capability). It takes typed arguments, never shells out, never
  touches the network, does exactly one thing, and exits.

Setup may elevate **once**; daily use never does.

### 🔒 HTTPS without the hassle

Yerd generates a local certificate authority and issues a leaf certificate per
site on demand, terminated by a hand-rolled `rustls` reverse proxy.
`sudo yerd elevate trust` adds the CA to your system trust store — after that,
every `.test` site is green-padlock valid. **No OpenSSL anywhere.**

### 🧠 One source of truth

The daemon owns state. The CLI and the GUI are both *clients* — they never
reimplement daemon logic, so the CLI and GUI can never disagree.

### 🧩 A clean, testable core

> **Pure logic lives in library crates. I/O and OS calls are pushed to the edges
> behind traits.**

Business logic is unit-tested with in-memory fakes; real filesystem, network,
process, and OS calls live behind traits (`ProcessSpawner`, `TrustStore`,
`ResolverInstaller`, `PortBinder`, `Clock`, …) with one implementation per OS.
The result: a large, fast test suite and behaviour that's identical across
platforms.

### 🔕 Local and quiet

Yerd makes no network calls except the ones you explicitly ask for (downloading
the PHP builds you install). PHP updates are **notify-only** — Yerd tells you when
a newer patch exists, but never installs anything behind your back.

---

## How it works

```
            ┌──────────────┐         .test domain
 browser ──▶│  yerdd        │◀── embedded DNS resolver (*.test → 127.0.0.1)
            │  reverse      │
            │  proxy        │── HTTPS termination via local CA (rustls + rcgen)
            └──────┬────────┘
                   │ FastCGI
            ┌──────▼────────┐
            │  PHP-FPM      │  one supervised pool per PHP version
            │  pools        │  (downloaded static builds)
            └───────────────┘

  yerd (CLI) ──IPC socket──▶ yerdd          sudo yerd elevate ──▶ yerd-helper
```

| Concern | Choice |
|---|---|
| Core language | Rust (edition 2021; core MSRV 1.77, GUI needs 1.85+) |
| TLS / local CA | `rustls` + `rcgen` (never OpenSSL) |
| Reverse proxy | hand-rolled `hyper` + `hyper-util` + `tokio-rustls` |
| DNS | `hickory-dns` embedded resolver for `*.test` |
| PHP runtime | `static-php-cli` builds, PHP-FPM per version |
| Services | native MySQL / MariaDB / PostgreSQL / Redis, supervised per-user (`yerd-services` + `yerd-supervise`) |
| IPC | Unix socket / Windows named pipe via `interprocess` |
| GUI | Tauri v2 + Vue 3 + TypeScript + Tailwind (`apps/yerd-gui`) |

---

## Roadmap

Shipping today (macOS + Linux): multi-version PHP, parked/linked `.test` sites,
HTTP + HTTPS with a local CA, the embedded DNS resolver, native database & cache
services (MySQL · MariaDB · PostgreSQL · Redis), `status`/`doctor`, and the
Debian package.

On the way:

- 🖥️ **Desktop GUI** — implemented in `apps/yerd-gui` (Tauri v2 tray app over the
  same daemon, a thin IPC client like the CLI); bundled as `.dmg`/`.AppImage`/`.deb`
  by the release pipeline. Code-signing/notarisation still to come.
- 🪟 **Windows support** — NRPT-based resolver, named-pipe IPC, system cert store,
  TCP-loopback PHP-FPM.
- 📦 **More packaging** — code-signing/notarisation, an Arch AUR package, and
  Fedora/openSUSE `.rpm`s. (`.dmg`/`.AppImage`/`.deb` + checksummed CLI artifacts
  are already built per release.)

---

## Development

```bash
# The full CI gate (all must pass):
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Build a .deb locally:
cargo xtask deb
```

The desktop GUI lives in `apps/yerd-gui` (Tauri v2 + Vue 3). It builds with a
newer toolchain than the core MSRV — `rust-toolchain.toml` pins **1.96** because
current Tauri v2 needs edition2024 (rustc ≥ 1.85) — and needs Node plus the
GTK/WebKit `-dev` system libraries. Setup, the `apt` one-liner, and
`npm run tauri dev` are documented in
[`apps/yerd-gui/README.md`](apps/yerd-gui/README.md).

**CI & releasing.** `.github/workflows/ci.yml` runs the gate above (plus the GUI
frontend tests) on every PR. To cut a release, bump the version everywhere and
push a tag — the pipeline builds and publishes **all** artifacts atomically (the
release stays a hidden draft until every file + `SHA256SUMS` is attached, then
flips public):

```bash
cargo xtask bump 2.0.2      # sets Cargo.toml + tauri.conf.json + package.json
git commit -am "release: v2.0.2" && git tag v2.0.2 && git push --follow-tags
```

`release.yml` then builds the CLI (`.deb` + tarballs) and GUI
(`.dmg`/`.AppImage`/`.deb`) for Linux (amd64 + arm64) and macOS (Apple Silicon).
A mismatched tag fails fast via `cargo xtask version-check`.

Conventions: `thiserror` in libraries / `anyhow` only at binary top level; no
`unwrap`/`expect`/`panic` outside tests (clippy-enforced); pure crates stay pure;
the IPC wire format is a versioned, byte-pinned contract.

---

## Lineage

Yerd v2 is a ground-up rewrite of **our own v1 package**
([`LumoSolutions/yerd`](https://github.com/LumoSolutions/yerd)) — the Go tool we
first built to scratch this itch. Shipping v1 taught us a lot, and we rebuilt Yerd
from scratch in Rust to make it cross-platform, rootless, and far easier to
maintain. v1 is reference-only: there's no command-surface or config-format
compatibility. Where v1 built PHP from source and leaned on `sudo` for most
operations, v2 ships prebuilt PHP and runs unprivileged.

---

## License

Licensed under either of MIT or the Apache License, Version 2.0, at your option.

Maintained by **Forjed** · <support@forjed.io> ·
[github.com/forjedio/yerd](https://github.com/forjedio/yerd)
