# Getting Started

This guide takes you from a clean machine to your first site live at
`https://my-app.test` - installed, served, and on the PHP version it needs.

Yerd runs entirely **as your user**. `sudo` shows up in exactly two
non-ongoing places: installing the system `.deb` (standard for any package),
and a single, optional, **one-time** setup step. Day-to-day use never touches
root.

::: info Supported platforms
Yerd ships prebuilt binaries for **Linux** (x86-64 and arm64) and **macOS**
(Apple Silicon). PHP itself is **not** bundled - Yerd downloads prebuilt static
PHP builds on demand when you run `yerd install php`, so the install stays tiny
and fast.
:::

::: warning Apple Intel not supported
Intel (x86-64) Macs are not supported at this time. macOS builds target Apple
Silicon (arm64) only.
:::

## Install

### Quick install (CLI + daemon)

The one-liner fetches the latest release, **verifies it against `SHA256SUMS`**,
and installs the three CLI/daemon binaries (`yerd`, `yerdd`, `yerd-helper`):

```sh
curl -fsSL https://raw.githubusercontent.com/forjedio/yerd/main/scripts/install.sh | sh
```

What it does, by platform:

- **Debian / Ubuntu** (where `dpkg` and `apt-get` are present): installs the
  system `.deb` (uses `sudo`).
- **Everything else** (other Linux, macOS): installs a tarball to
  `~/.local/bin` - **no sudo**.
- **Non-Debian systemd distros** (Arch/Omarchy, Fedora, openSUSE, …): also
  drops in a `yerd` **user service**, so the daemon starts the same way as on
  the `.deb`.

You can tune the installer with environment variables:

| Variable | Effect | Example |
|---|---|---|
| `YERD_VERSION` | Pin an exact version instead of latest | `YERD_VERSION=2.0.2` |
| `YERD_BIN_DIR` | Install dir for the tarball path | `YERD_BIN_DIR=~/bin` |
| `YERD_REPO` | Override the GitHub repo | `YERD_REPO=forjedio/yerd` |

```sh
YERD_VERSION=2.0.2 curl -fsSL https://raw.githubusercontent.com/forjedio/yerd/main/scripts/install.sh | sh
```

::: tip Add the install dir to your PATH
On the tarball path the installer prints a reminder if `~/.local/bin` (or your
`YERD_BIN_DIR`) isn't already on your `PATH`. Add it, then re-open your shell.
:::

### Manual download

Every [GitHub Release](https://github.com/forjedio/yerd/releases) attaches
prebuilt artifacts plus a `SHA256SUMS` manifest. Pick the right one:

| Platform | CLI artifact |
|---|---|
| Debian / Ubuntu (amd64 · arm64) | `yerd_<ver>_amd64.deb` · `yerd_<ver>_arm64.deb` → `sudo dpkg -i …` |
| Arch · Fedora · other Linux (rootless) | `yerd-<ver>-{x86_64,aarch64}-generic-linux-gnu.tar.gz` |
| macOS (Apple Silicon) | `yerd-<ver>-aarch64-apple-darwin.tar.gz` |

Always verify the download against the release's `SHA256SUMS` before using it:

```sh
sha256sum -c SHA256SUMS --ignore-missing
# macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
```

Then for the tarball, unpack and place the three binaries on your `PATH`:

```sh
tar -xzf yerd-<ver>-<triple>.tar.gz
install -m 0755 yerd yerdd yerd-helper ~/.local/bin/
```

::: tip How the `.deb` binds 80/443
The `.deb`'s post-install grants `yerdd` the `cap_net_bind_service` capability
(via `setcap`) so the **unprivileged** daemon can bind ports 80/443, and
re-applies it on every upgrade. If that capability isn't available, Yerd falls
back to `8080`/`8443` automatically - and `yerd doctor` tells you.
:::

### Desktop GUI (optional)

The tray app ships as separate bundles on the same release. It's a **client of
the daemon** - install the CLI/daemon above too, so `yerdd` is present.

| Platform | GUI artifact | Install |
|---|---|---|
| macOS (Apple Silicon) | `Yerd_<ver>_aarch64.dmg` | open, drag to Applications |
| Linux | `Yerd_<ver>_amd64.AppImage` | `chmod +x` and run |
| Linux | `Yerd_<ver>_amd64.deb` | `sudo dpkg -i …` |

::: warning Unsigned for now
macOS warns on first launch - right-click → **Open**, or clear the quarantine
flag: `xattr -dr com.apple.quarantine /Applications/Yerd.app`. See the
[Desktop App](./desktop-app) guide for more.
:::

### From source

No system package, and no `sudo` to install - build the binaries and drop them
on your `PATH`:

```sh
git clone https://github.com/forjedio/yerd
cd yerd
cargo build --release -p yerd -p yerdd -p yerd-helper
install -Dm755 target/release/{yerd,yerdd,yerd-helper} -t ~/.local/bin
```

`cargo xtask deb` packages a `.deb` instead. See
[Building from Source](../developer/building) for toolchain requirements.

## Start the daemon

`yerdd` is the per-user daemon that owns all runtime state - the reverse proxy,
the embedded DNS responder, and the PHP-FPM pools. Nothing works until it's
running, and it always runs as **you**, never root.

**Linux with a user service** (the `.deb`, or the installer's user unit on
non-Debian distros):

```sh
systemctl --user enable --now yerd
```

If the installer wrote the unit by hand, reload first:

```sh
systemctl --user daemon-reload && systemctl --user enable --now yerd
```

**macOS, or any rootless setup** - run the daemon directly. With no privileged
ports configured it binds `8080`/`8443` out of the box:

```sh
yerdd serve &
```

`yerdd serve` takes a couple of optional flags:

| Flag | Effect |
|---|---|
| `-v`, `-vv` | Increase log verbosity (`-v` → debug, `-vv` → trace) |
| `-c`, `--config <path>` | Override the config file location |

::: tip Confirm it's alive
`yerd ping` checks the daemon is reachable over its per-user socket - a quick
way to confirm the daemon came up before going further.
:::

## One-time setup (optional but recommended)

For the full experience - browser `*.test` resolution and trusted HTTPS - run
the one privileged step. It's the only command that uses root, and each piece
is independent and named:

```sh
sudo yerd elevate                       # all three, in order
# …or pick pieces:
sudo yerd elevate trust                 # trust the local CA in the system store
sudo yerd elevate resolver              # route *.test → yerd's DNS responder
sudo yerd elevate ports                 # allow the daemon to bind 80/443
```

`sudo yerd elevate` with no argument runs **trust → resolver → ports** in that
order. Under the hood the CLI only orchestrates as root: it reads facts from
your running daemon, then hands each privileged operation to the audited
`yerd-helper` one-shot binary. After this, `yerd` never touches root again.

The three pieces differ a little by OS:

- **trust** - adds Yerd's local CA to the system trust store, so every secured
  `.test` site gets a green padlock.
- **resolver** - routes `*.test` lookups to Yerd's embedded responder. On Linux
  without `systemd-resolved` this step is skipped, and Yerd tells you to point
  `/etc/resolv.conf` at its DNS address manually.
- **ports** - on **Linux** grants `cap_net_bind_service` to `yerdd` (you'll be
  prompted to restart the daemon for 80/443 to take effect; note that package
  upgrades reset `setcap`, so re-run `elevate ports` afterwards). On **macOS**
  it installs a `pf` redirect `80 → 8080`, `443 → 8443`, which goes live
  immediately with no daemon restart.

::: warning Start the daemon first
`elevate` reads configuration from your **running** daemon (the DNS address,
TLD, and CA path). If the daemon isn't up, it'll tell you to start it and
re-run.
:::

To reverse any of this later, use `sudo yerd unelevate [trust|resolver|ports]`.
On Linux the `setcap` grant can't be cleanly dropped, so `unelevate ports`
prints the manual `setcap -r` command rather than running it. See the
[Elevation & Privileges](./elevation) guide for the full model.

::: tip Skipping elevation entirely
You can run completely rootless and just drive sites on `http://127.0.0.1:8080`
with a `Host:` header. Elevation only buys you browser-native `.test` names and
a trusted CA. See [DNS & .test Domains](./dns) and
[HTTPS & Certificates](./https).
:::

## Serve your first site

Yerd serves sites two ways. **Park** a directory and every child folder becomes
a site; or **link** a single project under a name you choose.

```sh
# Park a directory - every sub-folder becomes <folder>.test
yerd park ~/Sites
#   ~/Sites/blog  ->  http://blog.test

# …or link a single project under a name you choose
yerd link my-app ~/code/my-app
#   ->  http://my-app.test
```

List everything Yerd knows about:

```sh
yerd sites                 # name, kind, PHP version, HTTPS, doc-root
yerd list parked           # the registered parked roots (incl. empty ones)
```

To stop serving, `yerd unlink <name>` removes a linked site, and
`yerd unpark <dir>` un-parks a directory (linked sites are untouched). See
[Sites](./sites) for the full lifecycle.

## Enable HTTPS

Turn HTTPS on for any site - Yerd issues a per-site certificate from its local
CA on demand:

```sh
yerd secure my-app
#   ->  https://my-app.test  (trusted, once the CA is trusted)
yerd unsecure my-app        # back to HTTP only
```

::: tip Green padlock
The certificate is only **trusted** by your browser after `sudo yerd elevate
trust` (above) has added the CA to your system store. Without it the site is
still served over HTTPS - your browser just warns about the unknown issuer.
Details in [HTTPS & Certificates](./https).
:::

## Choose a PHP version

Install one or more PHP versions (downloaded as prebuilt static builds), then
point sites at them. Set a **global** default with one argument, or pin a single
**site** with two:

```sh
yerd install php 8.5        # download + install PHP 8.5
yerd use 8.5                # set the global default version
yerd use my-app 8.3         # pin this one site to 8.3
```

Manage installed versions:

```sh
yerd list php               # installed versions + the global default
yerd list php --check       # also refresh "update available" status (network)
yerd list php --available   # versions installable from the distribution
yerd update php             # update every installed version
yerd update php 8.3         # …or just one
yerd uninstall php 8.3      # remove a version (blocked if a site uses it)
yerd restart php 8.5        # restart a pool (omit version = all running pools)
```

You can also set global PHP ini defaults that apply to every installed version:

```sh
yerd set php memory_limit 512M
yerd unset php memory_limit       # reset to PHP's built-in value
```

::: info Notify-only updates
Yerd never installs PHP updates behind your back. `yerd list php` annotates when
a newer patch exists; you decide when to run `yerd update php`.
:::

More in [PHP Versions](./php-versions).

## Check health

Yerd is self-diagnosing. Use these any time something looks off:

```sh
yerd status        # snapshot: daemon, ports, DNS, CA trust, PHP pools (PID/RAM), load
yerd doctor        # diagnose common problems
yerd doctor fix    # attempt the safe, unprivileged repairs (e.g. restart a crashed pool)
```

::: tip JSON everywhere
Add `--json` to **any** command for machine-readable output - handy for scripts
or editor integrations.
:::

## Putting it all together

A complete first run, start to finish:

```sh
# install + start
curl -fsSL https://raw.githubusercontent.com/forjedio/yerd/main/scripts/install.sh | sh
systemctl --user enable --now yerd      # macOS/rootless: yerdd serve &
sudo yerd elevate                       # one-time: trust CA, resolver, ports

# PHP + a site
yerd install php 8.5
yerd use 8.5
yerd link my-app ~/code/my-app
yerd secure my-app

# verify
yerd status
```

Open `https://my-app.test` in your browser - that's it.

## Where to next

- [Sites](./sites) - parking, linking, and per-site settings in depth.
- [PHP Versions](./php-versions) - pools, ini defaults, and updates.
- [HTTPS & Certificates](./https) and [DNS & .test Domains](./dns).
- [The Daemon](./daemon) and [Elevation & Privileges](./elevation).
- [CLI Reference](../reference/cli/) and
  [Configuration Reference](../reference/configuration) for the full surface.
