# Daemon control

| Command | Description | Example |
| --- | --- | --- |
| `yerd restart daemon` | Restart the daemon itself. | `yerd restart daemon` |
| `yerd daemon install` | Register the daemon to start at boot and start it now ([headless installs](../../guide/headless) only). | `yerd daemon install` |
| `yerd daemon start` | Start the registered daemon. | `yerd daemon start` |
| `yerd daemon stop` | Stop the running daemon. | `yerd daemon stop` |

## `restart daemon`

```sh
yerd restart daemon
```

::: warning
`yerd restart daemon` briefly interrupts all sites, and this command itself, since it's a client of the daemon it's restarting.
:::

`yerd update --yes` (see [Self-Update](./update)) also restarts the daemon, as the final step of installing a new Yerd version - that restart goes through the same OS service mechanics as `restart daemon`, not a fresh `yerdd` process started from scratch.

## `daemon install|start|stop`

With the desktop app, the app registers and controls the daemon (see [The Daemon](../../guide/daemon) for how `yerdd` is launched and supervised, and the [yerdd binary page](../../developer/binaries/yerdd) for internals). A [headless install](../../guide/headless) has no app, so these three local commands do the same job. They don't talk to the daemon, so they work while it's stopped.

```sh
# Linux: as your user, never with sudo
yerd daemon install
yerd daemon stop
yerd daemon start

# macOS: install and start need root (the job is a system LaunchDaemon)
sudo ~/.yerd/bin/yerd daemon install
yerd daemon stop
sudo ~/.yerd/bin/yerd daemon start
```

| Verb | Linux | macOS |
| --- | --- | --- |
| `install` | Writes the systemd user unit `yerd.service` (under `$XDG_CONFIG_HOME/systemd/user`, or `~/.config/systemd/user`), runs `systemctl --user enable --now yerd`, and enables lingering so it starts at boot with nobody logged in. Stops a hand-started `yerdd` first. Refuses under `sudo`. | Run with `sudo`. Has `yerd-helper` write `/Library/LaunchDaemons/dev.yerd.daemon.plist` (owned by root, `UserName` = you) and load it, so it starts now and at boot as your user. Refuses while a hand-started `yerdd` is running. |
| `start` | Starts the `yerd` user service. | Run with `sudo`: reloads the LaunchDaemon, which starts it. Refuses if the registered job runs a different `yerdd` or a different account, rather than taking it over. Without `sudo` and before `install`, it refuses and points you at `sudo <path>/yerd daemon install`. |
| `stop` | Stops the daemon. It stays down until started or the next boot. | Same. Run without `sudo`. |

Use the absolute path with `sudo`: its `secure_path` doesn't include `~/.yerd/bin`.

`yerd daemon install` only runs from a headless install (one with a `yerd-install-receipt` next to the binaries). It refuses, without changing anything, when:

- the `yerd` you ran belongs to the desktop app, which manages the daemon itself;
- the desktop app (in `/Applications` or `~/Applications`), or its daemon registration (a loaded launchd agent or its plist), is present;
- on macOS, the LaunchDaemon is already registered for another account;
- on Linux, `yerd.service` already exists and runs a different `yerdd`, or wasn't written by Yerd;
- on Linux, the install path contains whitespace, `%`, `$`, quotes or backslashes (systemd's `ExecStart=` would rewrite it), so reinstall with `YERD_INSTALL_DIR` set to a plain path;
- on Linux, no systemd user manager is reachable or the unit directory isn't writable. These are checked before a running daemon is stopped, so it keeps running.

There is no `daemon uninstall`: [`yerd uninstall`](./uninstall) removes the registration along with everything else.

| Exit code | Meaning |
| --- | --- |
| `0` | Done. |
| `1` | The service manager failed. A `yerd-helper` failure (macOS) exits with the helper's own code. |
| `64` | `stop` (or Linux `start`) was run under `sudo`. |
| `75` | The daemon didn't exit in time (`stop`, or the hand-started daemon `install` stops on Linux). |
| `77` | Needs root (macOS `install`/`start` without `sudo`), or must not be root (Linux `install` under `sudo`). |
| `78` | Not a headless install, a desktop app install or registration is in the way, the macOS job belongs to another install or account, no job is registered yet (macOS `start`), or the Linux registration can't go ahead (an unsupported character in the path, no systemd user manager, unit dir not writable). |
