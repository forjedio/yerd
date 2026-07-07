<div align="center">
  
<img src="https://yerd.app/images/social-card.png" alt="Yerd - Local PHP, without the friction. A fast, rootless, open-source local PHP development environment for macOS and Linux: serve .test domains over HTTP and HTTPS, run a different PHP version per site, and manage it all from one tiny daemon - no Docker, no sudo for everyday work, no subscription." width="840" />

</div>

---

<div align="center">

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Platforms: macOS · Linux](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux-success.svg)](#installation)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Docs](https://img.shields.io/badge/docs-yerd.app-6366f1.svg)](https://yerd.app)
[![Docs deploy](https://github.com/forjedio/yerd/actions/workflows/docs.yml/badge.svg)](https://github.com/forjedio/yerd/actions/workflows/docs.yml)

📖 **[Read the documentation at yerd.app →](https://yerd.app)**

</div>

---

<div align="center">

<img src="https://yerd.app/images/overview-dark.png" alt="The Yerd desktop app showing the Overview dashboard in dark mode" width="840" />

</div>

Yerd is a **single desktop app** for macOS and Linux - the daemon, the `yerd`
CLI, and a privileged one-shot helper are all bundled inside it; nothing is
downloaded at runtime. The tray-first GUI is a thin client over a tiny (~8 MB)
background daemon: a live dashboard of what's running, with one-click control of
PHP versions, `.test` sites, databases, mail capture, live Laravel dumps, public
sharing, and per-site HTTPS. A guided
[onboarding journey](https://yerd.app/guide/getting-started#first-launch-the-onboarding-journey)
gets you from install to serving sites in a couple of minutes. Every button maps to the same
daemon the [CLI](https://yerd.app/reference/cli/) drives, so the app and the
terminal never drift out of sync - and if you prefer the keyboard, `yerd` does
everything the app does.

---

## Why Yerd?

- 🚀 **Zero-config sites** - drop a project in a parked folder, it's live at `<name>.test`, framework web root auto-detected.
- 🔒 **Trusted HTTPS** per site from a local CA - no `mkcert`, no browser warnings.
- 🐘 **Multiple PHP versions**, pinned per site.
- 🗄️ **Native MySQL · MariaDB · PostgreSQL · Redis** - no Docker.
- 📬 **Mail capture** and a live **Laravel dump / query inspector**, built in.
- 🌍 **Share any site publicly** in one click via Cloudflare Tunnel - no ngrok account, no open ports.
- 🧰 **Composer, Node, and Bun** installed on demand as standalone binaries - no system package manager.
- 🪶 **One ~8 MB daemon** - no containers, no VM, no Electron.
- 🛡️ **Rootless** - setup elevates once; daily use never does.
- 🔌 **Works without admin** - can't route `.test`? Reach any site at `http://localhost:8080/~<name>.test`.
- 🔍 **Self-diagnosing** with `yerd status` and `yerd doctor`.

---

## Yerd vs. Herd vs. Lerd

|  | Laravel Herd | Lerd | **Yerd** |
|---|:---:|:---:|:---:|
| Free | ✅ (Pro is paid) | ✅ | ✅ |
| Open source | ❌ | ✅ | ✅ |
| Linux support | ❌ | ✅ | ✅ |
| macOS support | ✅ | ✅ | ✅ |
| Windows support | ✅ | ❌ | ❌ * |
| Automatic `.test` domains | ✅ | ✅ | ✅ |
| HTTPS with a trusted local CA | ✅ | ✅ | ✅ |
| Multiple PHP versions | ✅ | ✅ | ✅ |
| PHP version **per site** | ✅ | ✅ | ✅ |
| First-class CLI | ✅ | ✅ | ✅ |
| Menu-bar / tray GUI | ✅ | ✅ | ✅ |
| Database & cache services (MySQL · MariaDB · PostgreSQL · Redis) | ✅ (Pro) | ✅ | ✅ |
| Local mail capture (catch outgoing email) | ✅ (Pro) | ✅ | ✅ |
| Laravel dump / query inspector | ✅ (Pro) | ✅ | ✅ |
| Share a site publicly (tunnel) | ✅ | ❌ | ✅ |
| Runs rootless day-to-day | ✅ | ✅ † | ✅ |
| **No** Docker / Podman / containers required | ✅ | ❌ | ✅ |
| Lightweight (no VM, no container images) | ✅ | ❌ | ✅ |
| Built-in health checks (`doctor`) | ❌ | ❌ | ✅ |
| Under the hood | Native app (nginx + dnsmasq) | Containers (rootless Podman) | Native Rust (`rustls` proxy + embedded DNS) |

<sub>❌\* = Windows isn't supported yet - it's planned (coming soon). Yerd runs today on macOS and Linux.</sub>
<br><sub>**Lerd** runs your stack in containers via **rootless Podman** (Linux +
macOS; no Docker) - so it trivially adds database/cache services, but it pulls and
runs container images rather than native processes. † Rootless by design on
Podman.</sub>
<br><sub>**On Laravel Valet:** Valet is the original macOS-only Laravel dev tool
(nginx + dnsmasq, installed via Homebrew/Composer). None of the three require it -
Herd is the native standalone successor that bundles its own nginx (and reuses
Valet's framework "drivers"), Lerd runs everything in containers, and Yerd uses
its own Rust proxy + DNS. No Valet, no Homebrew.</sub>

---

## Installation

Yerd is a **single desktop app** - the daemon (`yerdd`), the `yerd` CLI, and the
privileged `yerd-helper` are all embedded inside it (nothing is downloaded at
runtime). Grab the latest **stable release** from the
[releases page](https://github.com/forjedio/yerd/releases):

| Platform | Download | Install |
|---|---|---|
| macOS (Apple Silicon) | `Yerd_MacOS_AppleSilicon_v<ver>.dmg` | open, drag to Applications |
| Linux · Debian/Ubuntu (x86-64) | `Yerd_Linux_x86_64_v<ver>.deb` | `sudo apt install ./Yerd_Linux_x86_64_v<ver>.deb` |
| Linux · Debian/Ubuntu (arm64) | `Yerd_Linux_Arm64_v<ver>.deb` | `sudo apt install ./Yerd_Linux_Arm64_v<ver>.deb` |
| Linux · Arch (x86-64) | `Yerd_Linux_x86_64_v<ver>.pkg.tar.zst` | `sudo pacman -U ./Yerd_Linux_x86_64_v<ver>.pkg.tar.zst` |
| Linux · Fedora (x86-64) | `Yerd_Linux_x86_64_v<ver>.rpm` | `sudo dnf install ./Yerd_Linux_x86_64_v<ver>.rpm` |
| Linux · Fedora (arm64) | `Yerd_Linux_Arm64_v<ver>.rpm` | `sudo dnf install ./Yerd_Linux_Arm64_v<ver>.rpm` |

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
puts it on your `PATH`; on **macOS** open *Settings → Terminal CLI → Install*.
Then the one-time
setup is available from the terminal too:

```bash
sudo yerd elevate    # trust the CA · route *.test · allow 80/443
```

---

## Quick start

Yerd is **GUI-first**: the desktop app drives everything from a few clicks. Each
step below shows the app, with the equivalent `yerd` CLI commands as an
alternative - both are clients of the same daemon, so anything you do in one
shows up in the other.

### 1. Install a PHP version

<div align="center">
<img src="https://yerd.app/images/php-dark.png" alt="The PHP page in the Yerd desktop app (dark mode)" width="820" />
</div>

On the **PHP** page, click **Install**, pick a version, and it becomes your
default (the first one always does). Manage updates and the global default from
the same page.

Alternative - the CLI:

```bash
yerd install php 8.5    # download + install a PHP version
yerd use 8.5            # make it the global default
```

### 2. Add and secure sites

<div align="center">
<img src="https://yerd.app/images/sites-dark.png" alt="The Sites page in the Yerd desktop app (dark mode)" width="820" />
</div>

On the **Sites** page, **park** a folder (every sub-folder becomes
`<name>.test`) or **link** a single project, flip HTTPS on or off per site, and
pick a PHP version per site - no commands.

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
<img src="https://yerd.app/images/doctor-dark.png" alt="The Doctor page in the Yerd desktop app (dark mode)" width="820" />
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
<img src="https://yerd.app/images/share-site-dark.png" alt="The Sites page row menu, with a Share publicly… action, in the Yerd desktop app (dark mode)" width="820" />
</div>

On the **Sites** page, hit **Share** for a one-off `*.trycloudflare.com` URL - or
connect your own Cloudflare account for a stable hostname on a domain you
manage. Outbound-only, no open ports, no `sudo`.

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
| **Sites** | `sites`, `park`, `unpark`, `link`, `unlink`, `root` |
| **HTTPS** | `secure`, `unsecure` |
| **PHP** | `use`, `install php`, `uninstall php`, `update php`, `restart php`, `list php`, `list parked`, `set php`, `unset php` |
| **Tooling** | `tools`, `install tool`, `uninstall tool`, `path install\|uninstall\|print` |
| **Services** | `services`, `service available\|install\|change-version\|uninstall\|start\|stop\|restart\|set-port\|logs` |
| **Databases** | `db list\|create\|drop\|backup\|restore` |
| **Mail** | `mail list\|show\|clear` |
| **Sharing** | `tunnel install\|share\|stop\|status\|login\|create\|delete\|list\|route\|set-host\|publish\|unpublish` |
| **Diagnostics** | `ping`, `status`, `doctor`, `doctor fix` |
| **Elevation** | `elevate [trust\|resolver\|ports]`, `unelevate` |
| **Daemon** | `restart daemon` |
| **Self-update** | `update [--yes] [--edge\|--stable]` |
| **Uninstall** | `uninstall [--yes]`, `uninstall php`, `uninstall tool` |

```bash
yerd park ~/Sites            # ~/Sites/blog -> http://blog.test
yerd secure my-app           # -> https://my-app.test
yerd install php 8.5 && yerd use my-app 8.5
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

Yerd runs as **three** pieces, and the GUI/daemon **never** run as root:

- **`yerdd`** - the unprivileged per-user daemon. It owns all runtime state and
  serves the proxy, DNS, and PHP-FPM pools.
- **`yerd`** - the CLI, a thin client that just talks to the daemon over a
  per-user socket.
- **`yerd-helper`** - a strict, auditable one-shot binary for the handful of
  operations that genuinely need root (trust the CA, configure the DNS resolver,
  grant the port capability). It takes typed arguments, never shells out, never
  touches the network, does exactly one thing, and exits.

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

Yerd makes no network calls except the ones you explicitly ask for - downloading
a PHP build, a dev tool, or `cloudflared` for sharing. Nothing is shared
publicly until you ask, and PHP updates are **notify-only**: Yerd tells you when
a newer patch exists, but never installs anything behind your back.

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
