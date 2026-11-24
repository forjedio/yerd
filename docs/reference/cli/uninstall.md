# Uninstall

`uninstall` has two shapes: remove a single **component**, or remove **yerd itself**.

| Command | Description |
| --- | --- |
| `yerd uninstall php <VERSION>` | Uninstall a PHP version (removes its files; blocked if a site uses it). See [PHP](./php). |
| `yerd uninstall tool <ID>` | Uninstall a dev tool (`composer`, `node`, `bun`, `pnpm`, `laravel`, `wp-cli`, `cloudflared`). `cloudflared` is refused while any tunnel is running and keeps your Cloudflare login and tunnels. See [Tooling](./tooling). |
| `yerd uninstall` | **Full uninstall** - remove yerd entirely from this machine. |
| `yerd uninstall --yes`, `-y` | Full uninstall without the confirmation prompt (for scripts / the desktop app). |

The component forms (`php` / `tool`) are daemon-mediated and documented on their own pages. The rest of this page covers the **bare `yerd uninstall`** - the full self-uninstall.

## Full uninstall

`yerd uninstall` with no subcommand removes everything yerd put on the machine. It is **local** (it doesn't go through the daemon - it stops the daemon as part of the teardown) and it **prompts for confirmation** before touching anything.

```sh
sudo yerd uninstall          # recommended: also reverts the elevate system changes
yerd uninstall               # without root: removes everything except the elevate changes
yerd uninstall --yes         # skip the prompt (non-interactive)
```

It removes, in order:

1. **System changes from [`elevate`](./elevation)** - the CA in the system trust store, the DNS resolver entry, and (macOS) the launchd port forwarders, plus any legacy `pf` redirect left by an older Yerd. **Root only** - see [Run it with `sudo`](#run-it-with-sudo).
2. **The daemon** - stops and disables the per-user service (systemd `--user` / launchd) and reaps the running `yerdd` (which in turn stops its PHP-FPM pools and any managed services).
3. **The PATH entry** - removes the yerd block from your shell startup files (the same block [`yerd path`](./tooling) manages).
4. **Config, data, and cache** - the `yerd.toml` config, installed PHP versions, dev tools, downloads, the local CA, and the runtime socket directory.
5. **The binaries** - `yerd`, `yerdd`, `yerd-helper`, and (macOS) `yerd-forward`. A package install is left for your package manager instead (`sudo apt purge yerd` on Debian/Ubuntu, `sudo pacman -R yerd` on Arch, `sudo dnf remove yerd` on Fedora) - yerd never `rm`s package-managed files.

When something can't be removed automatically, it's listed at the end as a leftover to handle manually.

## Headless installs

For a [headless install](../../guide/headless), run it with the absolute path, since `sudo` won't find `~/.yerd/bin` on its `PATH`:

```sh
sudo ~/.yerd/bin/yerd uninstall
```

On top of the steps above:

- **macOS:** under `sudo`, the helper first removes the boot-time LaunchDaemon that `yerd daemon install` registered (`system/dev.yerd.daemon`, `/Library/LaunchDaemons/dev.yerd.daemon.plist`). If that fails, uninstall stops there (exit `1`) without removing anything else, so the helper is still around to retry. Without root it can't remove the job, and launchd would restart a killed daemon, so `yerd uninstall` refuses up front (exit `77`, nothing changed) and tells you to run `sudo <path>/yerd uninstall`.
- **Linux:** the `yerd` systemd user unit is stopped, disabled and removed like the desktop app's, from wherever systemd loaded it (asked before disabling it, so a custom `$XDG_CONFIG_HOME` is found even under `sudo`), `$XDG_CONFIG_HOME/systemd/user` (when set) and `~/.config/systemd/user`. A unit file that can't be removed is listed in the summary. Lingering is left enabled, because other user services may rely on it; the command reminds you, and `sudo loginctl disable-linger <user>` turns it off.
- The `yerd-install-receipt` is removed with the binaries, along with any other `yerd-*` file a later update added there, and so is the install directory (`~/.yerd/bin`) once it's empty. Files without the `yerd-` prefix are left alone, so a shared `YERD_INSTALL_DIR` keeps your own files.

## Run it with `sudo`

The `elevate` changes (trust store, resolver, ports) need root to reverse - and they **can't be reversed after yerd is uninstalled**, because the `yerd-helper` binary is gone. So:

- **`sudo yerd uninstall`** reverts those system changes, then removes everything else (resolved for your real user, even under `sudo`).
- **`yerd uninstall`** (no root) prints a clear warning, lists the exact manual commands to undo the system changes later, and then removes everything else.

::: tip Undo elevation first, or just use sudo
If you started without root, the simplest path is to run `sudo yerd uninstall` so the trust/resolver/port changes are cleaned up automatically. Otherwise follow the printed manual steps (they include your TLD and the CA fingerprint so the leftover CA stays identifiable). See [Elevation & Privileges](../../guide/elevation).
:::

## Confirmation & non-interactive use

You'll be asked to confirm before anything is removed. Pass `--yes` (`-y`) to skip the prompt in scripts. With no TTY and no `--yes`, the command refuses rather than destroying anything silently.

## Platform support

Full uninstall is supported on **macOS** and **Linux**. The desktop app itself (the `.app` / `.deb` / `.pkg.tar.zst` / `.rpm`) is removed the usual way for your platform - drag to Trash on macOS, or `sudo apt purge yerd` (Debian/Ubuntu) / `sudo pacman -R yerd` (Arch) / `sudo dnf remove yerd` (Fedora) on Linux.

## See also

- [Elevation](./elevation) - the `elevate` / `unelevate` changes that the full uninstall reverts under `sudo`.
- [PHP](./php) and [Tooling](./tooling) - the component `uninstall php` / `uninstall tool` forms.
- [Getting Started → Uninstall](../../guide/getting-started#uninstall).
