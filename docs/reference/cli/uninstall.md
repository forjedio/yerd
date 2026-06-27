# Uninstall

`uninstall` has two shapes: remove a single **component**, or remove **yerd itself**.

| Command | Description |
| --- | --- |
| `yerd uninstall php <VERSION>` | Uninstall a PHP version (removes its files; blocked if a site uses it). See [PHP](./php). |
| `yerd uninstall tool <ID>` | Uninstall a dev tool (`composer`, `node`, `bun`). See [Tooling](./tooling). |
| `yerd uninstall` | **Full uninstall** — remove yerd entirely from this machine. |
| `yerd uninstall --yes`, `-y` | Full uninstall without the confirmation prompt (for scripts / the desktop app). |

The component forms (`php` / `tool`) are daemon-mediated and documented on their own pages. The rest of this page covers the **bare `yerd uninstall`** — the full self-uninstall.

## Full uninstall

`yerd uninstall` with no subcommand removes everything yerd put on the machine. It is **local** (it doesn't go through the daemon — it stops the daemon as part of the teardown) and it **prompts for confirmation** before touching anything.

```sh
sudo yerd uninstall          # recommended: also reverts the elevate system changes
yerd uninstall               # without root: removes everything except the elevate changes
yerd uninstall --yes         # skip the prompt (non-interactive)
```

It removes, in order:

1. **System changes from [`elevate`](./elevation)** — the CA in the system trust store, the DNS resolver entry, and (macOS) the `pf` port redirect. **Root only** — see [Run it with `sudo`](#run-it-with-sudo).
2. **The daemon** — stops and disables the per-user service (systemd `--user` / launchd) and reaps the running `yerdd` (which in turn stops its PHP-FPM pools and any managed services).
3. **The PATH entry** — removes the yerd block from your shell startup files (the same block [`yerd path`](./tooling) manages).
4. **Config, data, and cache** — the `yerd.toml` config, installed PHP versions, dev tools, downloads, the local CA, and the runtime socket directory.
5. **The binaries** — `yerd`, `yerdd`, and `yerd-helper`. A package install is left for your package manager instead (`sudo apt purge yerd` on Debian/Ubuntu, `sudo pacman -R yerd` on Arch) — yerd never `rm`s package-managed files.

When something can't be removed automatically, it's listed at the end as a leftover to handle manually.

## Run it with `sudo`

The `elevate` changes (trust store, resolver, ports) need root to reverse — and they **can't be reversed after yerd is uninstalled**, because the `yerd-helper` binary is gone. So:

- **`sudo yerd uninstall`** reverts those system changes, then removes everything else (resolved for your real user, even under `sudo`).
- **`yerd uninstall`** (no root) prints a clear warning, lists the exact manual commands to undo the system changes later, and then removes everything else.

::: tip Undo elevation first, or just use sudo
If you started without root, the simplest path is to run `sudo yerd uninstall` so the trust/resolver/port changes are cleaned up automatically. Otherwise follow the printed manual steps (they include your TLD and the CA fingerprint so the leftover CA stays identifiable). See [Elevation & Privileges](../../guide/elevation).
:::

## Confirmation & non-interactive use

You'll be asked to confirm before anything is removed. Pass `--yes` (`-y`) to skip the prompt in scripts. With no TTY and no `--yes`, the command refuses rather than destroying anything silently.

## Platform support

Full uninstall is supported on **macOS** and **Linux**. The desktop app itself (the `.app` / `.deb` / `.pkg.tar.zst`) is removed the usual way for your platform — drag to Trash on macOS, or `sudo apt purge yerd` (Debian/Ubuntu) / `sudo pacman -R yerd` (Arch) on Linux.

## See also

- [Elevation](./elevation) — the `elevate` / `unelevate` changes that the full uninstall reverts under `sudo`.
- [PHP](./php) and [Tooling](./tooling) — the component `uninstall php` / `uninstall tool` forms.
- [Getting Started → Uninstall](../../guide/getting-started#uninstall).
