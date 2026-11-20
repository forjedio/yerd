<div align="center">
  
<img src="https://yerd.app/images/social-card.png?v=2" alt="Yerd - Local PHP, without the weight. Every project in a parked folder is live on its own .test domain - HTTPS, per-site PHP, databases and mail. One tiny Rust daemon, and no Docker anywhere." width="840" />

</div>

---

<div align="center">

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Platforms: macOS · Linux](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux-success.svg)](#installation)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Docs](https://img.shields.io/badge/docs-yerd.app-3a63ee.svg)](https://yerd.app)
[![Docs deploy](https://github.com/forjedio/yerd/actions/workflows/docs.yml/badge.svg)](https://github.com/forjedio/yerd/actions/workflows/docs.yml)

📖 **[Read the documentation at yerd.app →](https://yerd.app)**

</div>

---

<div align="center">

<picture>
  <source media="(prefers-color-scheme: light)" srcset="https://yerd.app/images/sites-light.png" />
  <img src="https://yerd.app/images/sites-dark.png" alt="The Sites page in the Yerd desktop app: parked and linked sites grouped in a list, beside the selected site's PHP and Node versions, HTTPS, framework and domains" width="840" />
</picture>

</div>

Yerd is a **single desktop app** for macOS and Linux - the daemon, the `yerd`
CLI, and a privileged one-shot helper are all bundled inside it. The GUI is a
thin client over a small (~16 MB) native background daemon: park a folder and
every project in it is live on its own `.test` domain, with one-click control of
PHP and Node versions, HTTPS, domains, reverse proxies, databases, mail capture,
live Laravel dumps, and public sharing. A guided
[onboarding journey](https://yerd.app/guide/getting-started#first-launch-the-onboarding-journey)
gets you from install to serving sites in a couple of minutes. Every button maps to the same
daemon the [CLI](https://yerd.app/reference/cli/) drives, so the app and the
terminal never drift out of sync - and if you prefer the keyboard, `yerd` does
everything the app does. Running it on a server? The
[headless install](#headless-servers) gives you just the CLI and daemon.

---

## Why Yerd?

- 🚀 **Zero-config sites** - drop a project in a parked folder, it's live at `<name>.test`, framework and web root auto-detected. New Laravel and WordPress sites in a few clicks.
- 🔒 **Trusted HTTPS** per site from a local CA - no `mkcert`, no browser warnings.
- 🐘 **Multiple PHP versions** (8.2 - 8.5, plus opt-in legacy 7.4 / 8.0 / 8.1), pinned per site, with per-version ini settings, custom extensions, and a one-click **Xdebug** toggle.
- 🟢 **Node versions per project** - picked up from `.nvmrc` / `.node-version`, or pinned per site.
- 🌐 **Domains, proxies and routing** - extra domains, subdomains and wildcards per site; reverse proxies to local services (Reverb, Vite, anything on a port); routing rules for nested front controllers and SPAs; custom response headers.
- 🗄️ **Native MySQL · MariaDB · PostgreSQL · Redis (Valkey) · Meilisearch** - plus per-site Laravel Reverb - no Docker.
- 📬 **Mail capture** and a live **Laravel dump / query inspector**, built in.
- 🌍 **Share any site publicly** in one click via Cloudflare Tunnel - a quick link, or a stable hostname on your own domain, with optional password.
- 📱 **LAN access** - open your `.test` sites from a phone or another machine on your network.
- 🧰 **Composer, Node, Bun, pnpm, the Laravel installer, and WP-CLI** installed on demand as standalone binaries - no system package manager.
- 🤖 **AI agents welcome** - a built-in MCP server lets Claude Code, Cursor and friends manage sites, databases, mail and dumps.
- 🏢 **Corporate-network friendly** - an outbound HTTP proxy (system, manual, or off) for every download and tool Yerd runs.
- 🪶 **One ~16 MB daemon** - no containers, no VM, no Electron.
- 🛡️ **Rootless** - setup elevates once; daily use never does.
- 🔌 **Works without admin** - can't route `.test`? Reach any site at `http://localhost:8080/~<name>.test`.
- 🔍 **Self-diagnosing** with `yerd status`, `yerd doctor`, and one-click fixes in the app.

---

## Yerd vs. Herd vs. Lerd

|  | Laravel Herd | Lerd | **Yerd** |
|---|:---:|:---:|:---:|
| Free | ✅ (Pro is paid) | ✅ | ✅ |
| Open source | ❌ | ✅ | ✅ |
| Linux support | ❌ | ✅ | ✅ |
| macOS support | ✅ | ✅ | ✅ |
| Windows support | ✅ | ✅ (WSL2) | ❌ * |
| Automatic `.test` domains | ✅ | ✅ | ✅ |
| HTTPS with a trusted local CA | ✅ | ✅ | ✅ |
| Multiple PHP versions | ✅ | ✅ | ✅ |
| PHP version **per site** | ✅ | ✅ | ✅ |
| First-class CLI | ✅ | ✅ | ✅ |
| Desktop / tray GUI | ✅ | ✅ | ✅ |
| Database & cache services (MySQL · PostgreSQL · Redis …) | ✅ (Pro) | ✅ | ✅ |
| Local mail capture (catch outgoing email) | ✅ (Pro) | ✅ | ✅ |
| Laravel dump / query inspector | ✅ (Pro) | ✅ | ✅ |
| Share a site publicly (tunnel) | ✅ | ✅ | ✅ |
| MCP server for AI agents | ✅ | ✅ | ✅ |
| Runs rootless day-to-day | ✅ | ✅ † | ✅ |
| **No** Docker / Podman / containers required | ✅ | ❌ | ✅ |
| Lightweight (no VM, no container images) | ✅ | ❌ | ✅ |
| Under the hood | Native app (nginx + dnsmasq) | Containers (rootless Podman) | Native Rust (`rustls` proxy + embedded DNS) |

<sub>❌\* = Windows isn't supported yet - it's planned. Yerd runs today on macOS and Linux.</sub>
<br><sub>**Lerd** runs nginx, PHP-FPM and your services as **rootless Podman**
containers (Linux and macOS; Windows through WSL2) - so it adds services easily,
but it pulls and runs container images rather than native processes, and Podman
needs a Linux VM on macOS. † Rootless by design on Podman.</sub>
<br><sub>**On Laravel Valet:** Valet is the original macOS-only Laravel dev tool
(nginx + dnsmasq, installed via Homebrew/Composer). None of the three require it -
Herd is the native standalone successor that bundles its own nginx (and reuses
Valet's framework "drivers"), Lerd runs everything in containers, and Yerd uses
its own Rust proxy + DNS. No Valet, no Homebrew.</sub>
<br><sub>Comparison as of November 2026, from each project's own documentation.</sub>

---

## Installation

Yerd is a **single desktop app** - the daemon (`yerdd`), the `yerd` CLI, the
privileged `yerd-helper`, and on macOS the `yerd-forward` port forwarder are all
embedded inside it. PHP, services, and tools are downloaded on demand, only when
you ask for them. Grab the latest **stable release** from the
[releases page](https://github.com/forjedio/yerd/releases) (`<ver>` is the
version with dashes, e.g. `2-1-0`):

| Platform | Download | Install |
|---|---|---|
| macOS (Apple Silicon) | `Yerd_MacOS_AppleSilicon_v<ver>.dmg` | open, drag to Applications |
| Linux · Debian/Ubuntu (x86-64) | `Yerd_Linux_x86_64_v<ver>.deb` | `sudo apt install ./Yerd_Linux_x86_64_v<ver>.deb` |
| Linux · Debian/Ubuntu (arm64) | `Yerd_Linux_Arm64_v<ver>.deb` | `sudo apt install ./Yerd_Linux_Arm64_v<ver>.deb` |
| Linux · Arch (x86-64) | `Yerd_Linux_x86_64_v<ver>.pkg.tar.zst` | `sudo pacman -U ./Yerd_Linux_x86_64_v<ver>.pkg.tar.zst` |
| Linux · Fedora (x86-64) | `Yerd_Linux_x86_64_v<ver>.rpm` | `sudo dnf install ./Yerd_Linux_x86_64_v<ver>.rpm` |
| Linux · Fedora (arm64) | `Yerd_Linux_Arm64_v<ver>.rpm` | `sudo dnf install ./Yerd_Linux_Arm64_v<ver>.rpm` |
| Servers (no GUI) | `…headless.tar.gz` | `curl -fsSL https://yerd.app/install.sh \| sh` - see [Headless](#headless-servers) |

> **Arch note.** If you have a leftover `/usr/bin/yerd` from the old v1 (Go)
> project, remove it first - pacman refuses to install over a file it doesn't
> own. Run `pacman -Syu` before installing/upgrading so the WebKit/GTK libraries
> the app links match (it's built against current Arch packages).

On first launch the app **starts its bundled daemon** - so on macOS setup is
essentially drag-and-drop. It then walks you through a **one-time**
`sudo yerd elevate` to trust the local CA, route `*.test`, and bind ports 80/443.
Everything after runs as your user - never as root.

### Terminal CLI

The `yerd` command ships with the app: on **Linux** the `.deb`/`.pkg.tar.zst`
puts it on your `PATH`; on **macOS** open *Settings → Integrations and security
→ Terminal CLI*. Then the one-time setup is available from the terminal too:

```bash
sudo yerd elevate    # trust the CA · route *.test · allow 80/443
```

### Headless (servers)

On a server you only reach over SSH, skip the desktop app and install just the
CLI, daemon, and helper - no GUI, no WebKit/GTK, no login session needed:

```bash
curl -fsSL https://yerd.app/install.sh | sh   # installs into ~/.yerd/bin, as your user
yerd daemon install                           # Linux: systemd user service, starts at boot
sudo ~/.yerd/bin/yerd daemon install          # macOS: LaunchDaemon that runs yerdd as you
```

Supports Linux x86-64/arm64 (glibc 2.34+ and systemd; the bundled database and
search engines need glibc 2.38+) and Apple Silicon macOS.
`yerd update` updates a headless install in place, and `yerd lan enable` plus
`yerd remote-setup` let other machines browse its sites over trusted HTTPS. See
[Headless Install](https://yerd.app/guide/headless) for options, elevation over
SSH, and uninstalling.

---

## Quick start

Yerd is **GUI-first**: the desktop app drives everything from a few clicks. Each
step below shows the app, with the equivalent `yerd` CLI commands as an
alternative - both are clients of the same daemon, so anything you do in one
shows up in the other.

### 1. Install a PHP version

<div align="center">
<picture>
  <source media="(prefers-color-scheme: light)" srcset="https://yerd.app/images/php-light.png" />
  <img src="https://yerd.app/images/php-dark.png" alt="The PHP page in the Yerd desktop app: installed versions beside PHP 8.4's settings, extensions and Xdebug switch" width="820" />
</picture>
</div>

On the **PHP** page, click **Install another version**, pick one, and it becomes
your default (the first one always does). Manage updates, the global default,
ini settings, extensions, and Xdebug from the same page.

Alternative - the CLI:

```bash
yerd install php 8.5    # download + install a PHP version
yerd use 8.5            # make it the global default
```

### 2. Add and secure sites

<div align="center">
<picture>
  <source media="(prefers-color-scheme: light)" srcset="https://yerd.app/images/site-link-folder-light.png" />
  <img src="https://yerd.app/images/site-link-folder-dark.png" alt="The Link an existing folder dialog on the Sites page: a folder, site name, PHP version and HTTPS toggle" width="820" />
</picture>
</div>

On the **Sites** page, **Park folder** (every sub-folder becomes `<name>.test`),
**link** a single project, or create a new Laravel or WordPress site with
**New site**. Then flip HTTPS, pick PHP and Node versions, and add domains per
site - no commands.

Alternative - the CLI:

```bash
yerd park ~/Sites            # ~/Sites/blog -> http://blog.test
yerd link my-app ~/code/my-app   # -> http://my-app.test
yerd secure my-app           # -> https://my-app.test (trusted local CA)
yerd use my-app 8.3          # pin just this site to a PHP version
```

Open `https://my-app.test` in your browser - that's it.

### 3. Check and fix your environment

<div align="center">
<picture>
  <source media="(prefers-color-scheme: light)" srcset="https://yerd.app/images/doctor-light.png" />
  <img src="https://yerd.app/images/doctor-dark.png" alt="The Doctor page in the Yerd desktop app" width="820" />
</picture>
</div>

The **Doctor** page checks your setup (CA trust, the `.test` resolver,
privileged ports, PHP, sites) and offers **one-click fixes**.

Alternative - the CLI:

```bash
yerd status        # what's running
yerd doctor        # diagnose problems
yerd doctor fix    # apply the safe fixes
```

### 4. Share a site publicly

<div align="center">
<picture>
  <source media="(prefers-color-scheme: light)" srcset="https://yerd.app/images/share-light.png" />
  <img src="https://yerd.app/images/share-dark.png" alt="The Share page in the Yerd desktop app: quick links and sites on your own domain, with a shared site's public URL and status" width="820" />
</picture>
</div>

On the **Share** page (or **Share publicly…** from a site's menu), share a site as
a one-off `*.trycloudflare.com` quick link - or connect your own Cloudflare
account for a stable hostname on a domain you manage, optionally behind a
password. Outbound-only, no open ports, no `sudo`.

Alternative - the CLI:

```bash
yerd tunnel install     # one-time: fetch cloudflared
yerd tunnel share app   # -> https://calm-river-1234.trycloudflare.com
yerd tunnel stop app    # take it back offline
```

---

## CLI command reference

| Group | Commands |
|---|---|
| **Sites** | `sites`, `park`, `unpark`, `link`, `unlink`, `root`, `front-controller`, `list parked` |
| **Domains & routing** | `domain list\|add\|remove\|primary\|reset`, `route add\|remove\|list`, `header set\|remove\|list` |
| **Proxies** | `proxy add\|edit\|remove\|list` |
| **HTTPS** | `secure`, `unsecure` |
| **PHP** | `use`, `install php`, `uninstall php`, `update php`, `restart php`, `list php`, `set php`, `unset php`, `php ext\|ini\|pool`, `xdebug on\|off\|status`, `coverage` |
| **Node** | `node use\|unset\|uninstall\|list` |
| **Tooling** | `tools`, `install tool`, `uninstall tool`, `exec`, `which`, `path install\|uninstall\|print` |
| **Services** | `services`, `service available\|install\|add\|remove\|change-version\|uninstall\|start\|stop\|restart\|set-port\|set-autostart\|set-site\|set\|unset\|overrides\|logs` |
| **Databases** | `db list\|create\|drop\|backup\|restore` |
| **Mail** | `mail list\|show\|clear` |
| **Sharing** | `tunnel install\|share\|stop\|status\|login\|create\|delete\|list\|route\|set-host\|password\|publish\|unpublish` |
| **LAN** | `lan enable\|disable\|status`, `remote-setup` |
| **AI agents** | `mcp` |
| **Network** | `network proxy` |
| **Diagnostics** | `ping`, `status`, `doctor`, `doctor fix` |
| **Elevation** | `elevate [trust\|resolver\|ports\|lan]`, `unelevate` |
| **Daemon** | `restart daemon`, `daemon install\|start\|stop` |
| **Self-update** | `update [--yes] [--edge\|--stable]` |
| **Uninstall** | `uninstall [--yes]`, `uninstall php`, `uninstall tool` |

```bash
yerd park ~/Sites            # ~/Sites/blog -> http://blog.test
yerd secure my-app           # -> https://my-app.test
yerd install php 8.5 && yerd use my-app 8.5
yerd domain add my-app '*.my-app.test'          # wildcard subdomains
yerd proxy add reverb http://127.0.0.1:8080     # -> http://reverb.test
yerd db create mysql my_app
yerd tunnel share my-app     # -> https://calm-river-1234.trycloudflare.com
yerd status                  # what's running
```

Add `--json` to any command for machine-readable output. Full details, flags,
and JSON shapes: [CLI reference →](https://yerd.app/reference/cli/)

---

## Principles

Yerd is built around a few deliberate decisions that make it safe, fast, and
maintainable.

### 🛡️ Rootless, with a tight privilege boundary

Yerd runs as **three** pieces (plus a tiny port forwarder on macOS), and the
GUI/daemon **never** run as root:

- **`yerdd`** - the unprivileged per-user daemon. It owns all runtime state and
  serves the proxy, DNS, and PHP-FPM pools.
- **`yerd`** - the CLI, a thin client that just talks to the daemon over a
  per-user socket.
- **`yerd-helper`** - a strict, auditable one-shot binary for the handful of
  operations that genuinely need root (trust the CA, configure the DNS resolver,
  grant the port capability on Linux or install the port forwarder on macOS). It
  takes typed arguments, never shells out, never touches the network, does
  exactly one thing, and exits.
- **`yerd-forward`** (macOS only) - a small relay that launchd starts **as your
  user** to carry ports 80/443 to the daemon's rootless ports. It never runs as
  root.

Setup may elevate **once**; daily use never does. And if you **can't** elevate at
all - a locked-down machine where `.test` can't be routed - sites stay reachable
over plain `http://localhost:8080/~<name>.test` (Yerd pins that origin to the
site, or shows a picker), so you're never blocked. See
[Localhost Access](https://yerd.app/guide/localhost-access).

### 🔒 HTTPS without the hassle

Yerd generates a local certificate authority and issues a leaf certificate per
site on demand, terminated by a hand-rolled `rustls` reverse proxy.
`sudo yerd elevate trust` adds the CA to your system trust store - after that,
every `.test` site is green-padlock valid. **No OpenSSL anywhere.**

### 🧠 One source of truth

The daemon owns state. The CLI and the GUI are both *clients* - they never
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

Yerd only reaches the network to download what you ask for (a PHP build, a
service, a dev tool, `cloudflared`) and to check for updates and fetch release
notes. Nothing is shared publicly until you ask, and updates are
**notify-only**: Yerd tells you when a newer version exists, but never installs
anything behind your back. Behind a corporate proxy? Every download and tool Yerd
runs goes through the [outbound proxy](https://yerd.app/guide/corporate-networks)
you configure.

---

## Lineage

Yerd v2 is a ground-up rewrite of **our own v1 package**
([`LumoSolutions/yerd`](https://github.com/LumoSolutions/yerd)) - the Go tool we
first built to scratch this itch. Shipping v1 taught us a lot, and we rebuilt Yerd
from scratch in Rust to make it cross-platform, rootless, and far easier to
maintain. v1 is reference-only: there's no command-surface or config-format
compatibility. Where v1 built PHP from source and leaned on `sudo` for most
operations, v2 ships prebuilt PHP and runs unprivileged.

---

## License

Licensed under the [MIT License](LICENSE.md).

A [Forjed](https://forjed.io) project · [github.com/forjedio/yerd](https://github.com/forjedio/yerd)
