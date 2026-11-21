---
description: Install Yerd without the desktop app - just the CLI and daemon - on a Linux or macOS server, and keep the daemon running at boot with nobody logged in.
---

# Headless Install (Servers)

The [desktop app](./getting-started) is the recommended way to run Yerd on your
own machine. On a **server** - a Linux VM, a box in a rack, a Mac mini you only
reach over SSH - you don't want a GUI, its WebKit/GTK dependencies, or a login
session. The **headless install** gives you just the parts that do the work:

| Binary | Role |
|---|---|
| `yerdd` | The [daemon](./daemon): proxy, DNS, PHP-FPM pools, services. |
| `yerd` | The [CLI](../reference/cli/) - everything the app can do, from a terminal. |
| `yerd-helper` | The audited one-shot helper for the few [privileged steps](./elevation). |
| `yerd-forward` | macOS only: the port forwarder behind `sudo yerd elevate ports`. |

It runs the same way as a desktop install: **rootless**. The daemon runs as
your user; `sudo` appears only for one-time setup steps.

::: info Supported platforms
Linux **x86-64** and **arm64** with **glibc 2.34 or newer** and `systemd`
(e.g. Ubuntu 22.04, Debian 12, RHEL/Rocky 9, Fedora, Arch, or later), and
macOS on **Apple Silicon**. musl distros such as Alpine, older glibc (Debian
11, Ubuntu 20.04), Intel Macs and Windows are not supported. The install
script checks the glibc version before downloading anything.
:::

## Install

Run the install script as **your normal user** (not root):

```sh
curl -fsSL https://yerd.app/install.sh | sh
```

It:

1. Detects your OS and architecture.
2. Downloads the matching headless tarball for the latest **stable** release.
3. Checks the download against the release's `SHA256SUMS`.
4. Installs the binaries into `~/.yerd/bin`.
5. If you re-ran the script to upgrade and this install's daemon is running,
   restarts it onto the new version: through its `yerd` systemd unit on Linux,
   in place on macOS. A daemon you started by hand on Linux is left running;
   restart it yourself.
6. Runs `yerd path install` so `yerd` (and the `php`/`composer` shims) are on
   your `PATH`. Open a new shell, or `source` your shell profile, to pick it up.

It refuses to run as root, refuses on Linux without glibc 2.34+, and refuses
if the desktop app is already installed
(see [Coexisting with the desktop app](#coexisting-with-the-desktop-app)).

### Options

Set these environment variables on the `sh` side of the pipe:

| Variable | Effect |
|---|---|
| `YERD_VERSION=<tag>` | Install a specific release (e.g. `v2.2.0`, or a release candidate such as `v2.2.0-rc.1`; the leading `v` is optional) instead of the latest stable. Downloads come from `files.yerd.app`. |
| `YERD_INSTALL_DIR=<dir>` | Install somewhere other than `~/.yerd/bin`. Pick a directory you own, so self-update never needs root. The path can't contain spaces, `%`, `$`, quotes or backslashes (systemd would rewrite them in the service's command line), so the script refuses it. |
| `YERD_NO_MODIFY_PATH=1` | Don't touch your shell profile; add the install directory to `PATH` yourself. |

```sh
curl -fsSL https://yerd.app/install.sh | YERD_VERSION=v2.2.0 sh
```

::: tip No stable headless build yet?
Without `YERD_VERSION`, the script only looks at the latest **stable** release
and never falls back to a release candidate. If that release has no headless
tarball for your platform, it stops and tells you to set `YERD_VERSION`.
:::

### Installing by hand

Each release on the [releases page](https://github.com/forjedio/yerd/releases)
carries one tarball per platform:

| Platform | Tarball |
|---|---|
| Linux (x86-64) | `Yerd_Linux_x86_64_v<ver>.headless.tar.gz` |
| Linux (arm64) | `Yerd_Linux_Arm64_v<ver>.headless.tar.gz` |
| macOS (Apple Silicon) | `Yerd_MacOS_AppleSilicon_v<ver>.headless.tar.gz` |

Each one unpacks to a `yerd/` directory holding the binaries and a small
`yerd-install-receipt` file. Check it against `SHA256SUMS` (and, if you have
[minisign](https://jedisct1.github.io/minisign/), its `.minisig`), then copy the
**whole** directory's contents, receipt included, into a directory you own:

```sh
tar -xzf Yerd_Linux_x86_64_v<ver>.headless.tar.gz
mkdir -p ~/.yerd/bin
cp yerd/* ~/.yerd/bin/
~/.yerd/bin/yerd path install
```

::: warning Keep the receipt
`yerd-install-receipt` is how Yerd knows it is a headless install. Self-update
uses it to fetch the headless tarball rather than the desktop app's package,
and `yerd daemon install` refuses to run without it. Keep it next to the
binaries. Without it, Yerd assumes a desktop install, and `yerd update`
refuses to update rather than install the desktop package over your
headless binaries. Reinstall with the script to restore it.
:::

## Run the daemon as a service

The CLI doesn't start the daemon on its own. Register it as a service once and
it starts now **and at every boot**, whether or not anyone logs in.

### Linux

```sh
yerd daemon install
```

As your user (never with `sudo`), this:

- writes the systemd user unit `~/.config/systemd/user/yerd.service`,
- runs `systemctl --user enable --now yerd`,
- enables **lingering** for your account (`loginctl enable-linger`), so your
  user services start at boot and keep running after you log out.

If a `yerdd` you started by hand is already running, it is stopped first.

Some distros only let an administrator enable lingering. If so, the command
says so; run the line it prints:

```sh
sudo loginctl enable-linger "$USER"
```

Manage it like any user service:

```sh
systemctl --user status yerd
journalctl --user -u yerd -f
```

### macOS

```sh
sudo ~/.yerd/bin/yerd daemon install
```

A login-session LaunchAgent (what the desktop app uses) doesn't run until
someone logs in at the Mac. So a headless install registers a **LaunchDaemon**
instead: a root-owned `/Library/LaunchDaemons/dev.yerd.daemon.plist`, written by
the audited `yerd-helper`, with `UserName` set to **you**. launchd starts it at
boot, but `yerdd` still runs as your user, never as root. The `sudo` is only for
writing that plist, the same model as `sudo yerd elevate ports`.

If a `yerdd` you started by hand is still running, the command refuses. Run
`yerd daemon stop` (as yourself, without `sudo`) first.

::: tip Why the absolute path?
`sudo` resets `PATH` to a fixed `secure_path` that doesn't include
`~/.yerd/bin`, so `sudo yerd …` won't find the binary. Use the full path for
every `sudo` command on this page.
:::

::: warning FileVault blocks unattended boot
With FileVault on (the default on new Macs), an Apple Silicon Mac waits for
the disk to be unlocked after a reboot, and the daemon can't start until it
is. On macOS 26 or later with Remote Login on, you can unlock it remotely by
logging in over SSH. On older macOS nothing runs, not even SSH, until someone
unlocks it at the Mac. Either way someone has to unlock it, so for a truly
unattended server turn FileVault off. To keep it on, reboot with
`sudo fdesetup authrestart`, which unlocks the disk once for the next boot.
:::

::: warning Keep sites out of protected folders
macOS privacy controls (TCC) guard `~/Documents`, `~/Desktop` and
`~/Downloads`. A background daemon can't show the "allow access" prompt, so
sites in those folders fail with permission errors. Keep projects elsewhere
(e.g. `~/Sites`), or grant `yerdd` **Full Disk Access** in System Settings →
Privacy & Security.
:::

### Start and stop

```sh
yerd daemon stop                     # stop it (it stays down until started)
yerd daemon start                    # Linux
sudo ~/.yerd/bin/yerd daemon start   # macOS: starting a LaunchDaemon needs root
yerd restart daemon                  # restart in place, no sudo on either OS
```

`yerd daemon stop` always runs as you, without `sudo`. The service still starts
at the next boot. On macOS, `sudo … daemon start` only starts a job registered
for this install and your account; another install's job is left alone.

## Set up PHP and sites

From here everything is the normal CLI. A minimal first run:

```sh
yerd install php 8.5
yerd use 8.5
yerd park ~/Sites
yerd status
```

See the [CLI Reference](../reference/cli/) for the rest.

## Elevation over SSH

`sudo yerd elevate` works over SSH, using the absolute path:

```sh
sudo ~/.yerd/bin/yerd elevate            # trust, resolver, ports
sudo ~/.yerd/bin/yerd elevate ports      # just 80/443
```

What each step does is covered in [Elevation & Privileges](./elevation). A few
things are specific to servers:

- **Ports on Linux.** `elevate ports` grants `~/.yerd/bin/yerdd` the
  `cap_net_bind_service` capability. File capabilities are wiped whenever the
  binary is replaced, so **after every self-update** the daemon drops back to
  `8080`/`8443` until you re-run `sudo ~/.yerd/bin/yerd elevate ports` and
  then `yerd restart daemon` (the capability applies when `yerdd` next
  starts). If the daemon had the capability before the update, the updater
  reminds you.
- **`nosuid` home directories.** If `/home` is mounted `nosuid`, the kernel
  ignores file capabilities there and the daemon stays on `8080`/`8443`. Either
  live with the high ports, or install to a directory on a filesystem without
  `nosuid` using `YERD_INSTALL_DIR`.
- **CA trust on macOS needs a local session.** Since macOS 11, trusting a
  certificate pops up a confirmation dialog, and an SSH session has no screen to
  show it on. So `elevate trust` can't complete there. This only affects
  HTTPS requests made *on the server itself*. Either trust the CA once from a
  local or Screen Sharing session, or skip it.

### Reaching sites from other machines

A headless server is usually browsed from another machine. Use
[LAN sharing](../reference/cli/lan) to expose `.test` sites to your network,
and `yerd remote-setup` to make each client device trust Yerd's CA and resolve
`.test`. That's how clients get HTTPS without browser warnings, whatever the
server itself trusts.

## Updating

```sh
yerd update          # check
yerd update --yes    # download, verify, install, restart
```

A headless install updates **in place**. The daemon downloads the headless
tarball for your platform and checks its SHA-256 and minisign signature. The
CLI checks the signature again, swaps each binary atomically in your install
directory (keeping a backup, and rolling back if any step fails), and restarts
the daemon. If a newer release adds a file to the tarball, it is installed
alongside the rest. Your install directory is yours, so no `sudo` is needed. Channels
(`--edge`/`--stable`) work as described in [Self-Update](../reference/cli/update).

On Linux, if you use ports 80/443, run `sudo ~/.yerd/bin/yerd elevate ports`
and then `yerd restart daemon` afterwards ([why](#elevation-over-ssh)).

## Coexisting with the desktop app

Run **either** a headless install **or** the desktop app on one machine, not
both. They share the same daemon, socket and config, so Yerd refuses to mix
them rather than guess:

- `install.sh` refuses when `Yerd.app` is in `/Applications` or
  `~/Applications` (macOS), or `/usr/bin/yerdd` exists (Linux).
- `yerd daemon install` refuses when the desktop app or its daemon registration
  is present.
- `yerd update` refuses if the CLI you ran and the daemon belong to different
  installs, and tells you which `yerd` to use.

To switch, [uninstall](#uninstall) one before installing the other.

## Uninstall

```sh
sudo ~/.yerd/bin/yerd uninstall
```

Like a desktop uninstall, this stops the daemon and removes your config, data
and the binaries in `~/.yerd/bin`, receipt included. On Linux it also removes
the `yerd` systemd user unit. Running it with `sudo` also reverts any `elevate`
changes and, on macOS, removes the `dev.yerd.daemon` LaunchDaemon.

On Linux you can run `yerd uninstall` without `sudo`. It removes everything it
can and prints the commands to revert the `elevate` changes yourself. On macOS,
once `yerd daemon install` has registered the LaunchDaemon, uninstall **requires**
`sudo`. Without it, the command refuses before changing anything, because
launchd would keep restarting the daemon while its files were being deleted.

On Linux, lingering is left enabled, because other user services may rely on
it. Turn it off yourself if nothing else needs it:

```sh
sudo loginctl disable-linger "$USER"
```

See the [Uninstall reference](../reference/cli/uninstall) for the full list.
