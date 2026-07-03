# The Daemon

`yerdd` is an unprivileged, per-user background process that owns everything Yerd does at runtime: the reverse proxy that answers `.test` requests, the embedded DNS responder for `*.test`, and the supervised PHP-FPM pools. The `yerd` CLI and the desktop app are thin clients that talk to it over a local socket; they never reimplement its logic.

::: info One source of truth
The daemon owns the config and the live routing table. When you run `yerd link` or `yerd secure`, the CLI sends a request, the daemon validates it, persists it to `yerd.toml`, and swaps its in-memory router. So the CLI and GUI can't disagree. See [Sites](./sites) and the [IPC Protocol](../developer/ipc-protocol).
:::

For internal architecture (task wiring, shutdown channel, lock ordering) see [yerdd internals](../developer/binaries/yerdd).

## What the daemon owns

`yerdd serve` brings up and supervises:

| Subsystem | What it does |
|---|---|
| Reverse proxy | A `hyper` + `tokio-rustls` proxy on the HTTP/HTTPS ports, routing each `.test` host to its site and PHP pool. See [HTTPS & Certificates](./https). |
| DNS responder | A loopback-only resolver answering `*.test`. See [DNS & .test Domains](./dns). |
| PHP-FPM pools | One supervised pool per installed PHP version, started on demand. See [PHP Versions](./php-versions). |
| IPC server | A Unix-socket listener for the CLI and desktop app. See [the desktop app](./desktop-app). |
| Update checker | Polls for newer PHP patch releases every 12 hours, and for newer Yerd releases roughly every 4 hours (checked immediately if stale when the desktop app launches). Notify-only - it never installs anything. |
| Local CA | Loads (or on first run generates) the local certificate authority used to issue per-site certs. |

All of this runs as you, never as root. The only operations needing privilege (trusting the CA, configuring the system resolver, granting the port capability) are handled once by a separate audited helper. See [Elevation & Privileges](./elevation).

## Running the daemon

There's one command: `serve`, which runs in the foreground.

```sh
yerdd serve
```

Two flags:

| Flag | Effect |
|---|---|
| `-v`, `--verbose` | Increase verbosity. `-v` is debug, `-vv` is trace. Repeatable. |
| `-c`, `--config <PATH>` | Use a config file at a custom path instead of the default `yerd.toml`. |

```sh
# Debug logging and a custom config
yerdd serve -v --config ~/my-yerd.toml
```

Running `yerdd` with no subcommand equals `yerdd serve` with defaults.

::: tip You usually don't run yerdd by hand
On a typical install, let the app (or your OS service manager) keep it running (below). The bare `yerdd serve &` form is handy for a from-source run and for debugging, where it binds `8080`/`8443`.
:::

### The CLI does not auto-start it

The `yerd` CLI is a pure client. If the daemon isn't running, commands fail fast instead of silently launching it:

```text
daemon not running - start `yerdd`
```

Start `yerdd` (via your service manager or `yerdd serve &`) before using the CLI. `yerd doctor` also flags a stopped daemon. See [Diagnostics](./diagnostics).

## Autostart

How autostart is wired depends on your platform.

### Linux: systemd user service

Yerd uses a systemd `--user` unit named `yerd`. The app writes it to `~/.config/systemd/user/yerd.service` when you start the daemon or enable "Run daemon at login" - you don't install it by hand. It looks like:

```ini
[Unit]
Description=Yerd local PHP development daemon

[Service]
Type=simple
ExecStart=/usr/bin/yerdd serve
Restart=on-failure

[Install]
WantedBy=default.target
```

Enable and start it as your user (never with `sudo`):

```sh
systemctl --user daemon-reload      # only for a freshly-dropped unit
systemctl --user enable --now yerd
```

`Restart=on-failure` brings the daemon back after a crash, but not after a clean exit (e.g. a deliberate stop).

::: tip Keep it running after logout
A user service stops when your last session ends unless lingering is enabled:

```sh
loginctl enable-linger "$USER"
```
:::

::: info Binding 80/443 unprivileged
On a Linux package install (the `.deb`'s post-install, or the Arch package's `.install` scriptlet), the step grants `yerdd` the `cap_net_bind_service` capability so the unprivileged daemon can bind 80/443, and re-applies it on every upgrade (the package manager replaces the binary, wiping file capabilities). Without the capability, the daemon falls back to `8080`/`8443` and `yerd doctor` tells you. See [Elevation & Privileges](./elevation).
:::

### macOS

The app **bundles the daemon** and registers it as a background **`SMAppService`** agent, so it shows up as **Yerd** in System Settings → General → Login Items → Allow in the Background (attributed to the app, with its icon - not to the signing team). Manage it from **Settings → "Run the Yerd daemon in the background"** in the app; the tray menu's Start/Stop/Restart control the running process for the current session.

::: tip First-time approval
The first time the daemon registers, macOS may ask you to enable Yerd in Login Items. The app shows a banner with a button that takes you straight there. A LaunchAgent runs as your user, matching Yerd's rootless model.
:::

For a from-source / terminal run without the app, start the daemon directly:

```sh
yerdd serve &
```

## Lifecycle: start, stop, restart

Under systemd (Linux):

```sh
systemctl --user start yerd        # start
systemctl --user stop yerd         # stop
systemctl --user restart yerd      # restart
systemctl --user status yerd       # is it running?
```

Run by hand, the daemon shuts down gracefully on `Ctrl-C` (`SIGINT`) or `SIGTERM`: it broadcasts a shutdown to every subsystem, gives each a brief window to wind down, stops the PHP-FPM pools, releases its lock, and exits.

```sh
# Foreground: press Ctrl-C
# Backgrounded with `yerdd serve &`:
kill "$(pgrep -x yerdd)"           # sends SIGTERM
```

### Restarting via the CLI

To bounce the daemon without your service manager:

```sh
yerd restart daemon
```

This briefly interrupts all sites (and the command's own connection). The daemon does a graceful teardown then re-execs itself in place (same PID, same arguments), so it works the same whether or not it's supervised.

::: tip Reloading config vs. restarting
Everyday changes rarely need a full restart. Site, PHP-version, and HTTPS changes go through IPC and take effect immediately as the daemon re-scans parked roots and swaps its router live. Use `yerd restart daemon` when you change something read only at startup, such as `dns_port` (which must stay fixed so an installed resolver config keeps pointing at it). See the [Configuration Reference](../reference/configuration).
:::

## Single-instance protection

Only one `yerdd` runs per user. At startup it hardens its runtime directory to `0o700` and takes an exclusive advisory lock on `<runtime>/yerd.lock` (`flock`-style on Linux/macOS). The lock is held for the daemon's lifetime and released on exit.

A second instance fails immediately rather than racing for the socket:

```text
another yerdd is already running (lock held at …/yerd.lock)
```

That's exit code 75 (`EX_TEMPFAIL`). Other startup failures use sysexits-style codes:

| Code | Meaning | Cause |
|---|---|---|
| `0` | Success | Clean shutdown |
| `70` | `EX_SOFTWARE` | Generic failure (DNS/proxy/PHP/IPC) |
| `71` | `EX_OSERR` | Platform or TLS error |
| `74` | `EX_IOERR` | Filesystem I/O error |
| `75` | `EX_TEMPFAIL` | Another `yerdd` is already running |
| `78` | `EX_CONFIG` | Config or core-validation error |

::: tip "Already running" but nothing's serving?
An old process is still holding the lock. Check `pgrep -x yerdd`, stop it, then start fresh. The runtime directory also holds the IPC socket (`yerd.sock`), locked to your user as the access boundary.
:::

## Logging

`yerdd` uses `tracing` and writes a compact log to stderr. Verbosity maps to the `-v` flags:

| Flag | Level |
|---|---|
| (none) | `INFO` |
| `-v` | `DEBUG` |
| `-vv` | `TRACE` |

At the default level the DNS server's per-query logging is capped at `WARN`. Otherwise it would log every inbound lookup (including routine `NXDomain` results for non-`.test` names your OS forwards) and flood the log. Raising verbosity lifts that cap so you can watch DNS traffic.

Where the log goes depends on how the daemon was started:

- Under systemd (Linux), stderr goes to the journal:

  ```sh
  journalctl --user -u yerd -f
  ```

- Run by hand, stderr prints to your terminal. Redirect for persistence:

  ```sh
  yerdd serve > ~/yerd.log 2>&1 &
  ```

::: info Diagnostics over raw logs
For a quick health picture, prefer `yerd status` (daemon, ports, DNS, CA trust, PHP pools with PID/RAM, load) or `yerd doctor` over reading logs. Both query the running daemon over IPC. See [Diagnostics](./diagnostics).
:::

## Where the daemon keeps its files

`yerdd` resolves a small set of per-user directories at startup (XDG-based on Linux, the equivalents on macOS):

| Directory | Holds |
|---|---|
| config | `yerd.toml` (the authoritative config) |
| data | The local CA (`ca.cert.pem`, `ca.key.pem`) and issued leaf certificates |
| state | Long-lived state |
| cache | Downloads and other regenerable files |
| runtime | The IPC socket (`yerd.sock`) and single-instance lock (`yerd.lock`) |

The runtime directory is security-sensitive: it's forced to `0o700` and the IPC socket is restricted to your user, since directory and socket permissions are the only access control on the socket. The CA private key is locked to its owner; the CA certificate is world-readable but never group/world-writable, so the trust helper accepts it.

::: warning Never run yerdd as root
Yerd is rootless by design. Running as root creates root-owned files in your config/data/runtime directories and breaks the privilege boundary. When a command needs privilege, Yerd elevates a tiny audited helper for that one step. See [Elevation & Privileges](./elevation).
:::

## See also

- [Getting Started](./getting-started) - install and first run
- [Diagnostics](./diagnostics) - `status` and `doctor`
- [Configuration Reference](../reference/configuration) - every `yerd.toml` key
- [CLI Reference](../reference/cli/) - the full `yerd` command surface
- [yerdd internals](../developer/binaries/yerdd) - startup wiring, shutdown channel, lock ordering
- [IPC Protocol](../developer/ipc-protocol) - how clients talk to the daemon
- Source: [`bin/yerdd` on GitHub](https://github.com/forjedio/yerd/tree/main/bin/yerdd)
