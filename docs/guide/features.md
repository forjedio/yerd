# Features

Yerd is a fast, rootless, open-source local PHP environment for macOS and Linux. It serves projects on `.test` domains over HTTP and HTTPS, runs a different PHP version per site, and manages it all from one small daemon. No Docker, no `sudo` for daily work, no subscription.

Each section below links to its full guide.

::: tip The shape of Yerd
Three pieces with a clear privilege boundary: `yerdd` (the unprivileged per-user daemon that owns all runtime state), `yerd` (the CLI), and `yerd-helper` (a one-shot binary for the few operations that need root). The daemon is the single source of truth; the CLI and desktop app are both clients of it.
:::

## Zero-config sites

Drop a project into a parked directory and it's live at `<folder>.test`, with every sub-folder becoming its own site. Or link a single directory under a name you choose. No per-site config, no web server to set up - Yerd even detects each project's framework and serves from the right web root (Laravel/Symfony `public/`, CakePHP `webroot/`, WordPress the project root, and so on), so a freshly cloned app just works.

```sh
yerd park ~/Sites          # ~/Sites/blog  ->  http://blog.test
yerd link my-app ~/code/my-app   # ->  http://my-app.test
yerd sites                 # list every known site (kind, PHP, HTTPS, served path)
yerd root my-app public    # override the served web root if detection misses
yerd unlink my-app         # remove a linked or parked site
```

[Sites guide →](./sites)

## Per-site PHP versions

Install as many PHP versions as you need, pin each site to the one it requires, and set a global default for the rest. PHP isn't bundled: Yerd downloads prebuilt static builds on demand, so the install stays small. Each version runs as its own supervised PHP-FPM pool.

```sh
yerd install php 8.5       # download + install a version
yerd use 8.5               # set the global default
yerd use my-app 8.3        # pin one site to a specific version
yerd list php --check      # list installed versions, flag available updates
```

[PHP versions guide →](./php-versions)

## Automatic HTTPS

Yerd runs a local certificate authority and issues a leaf certificate per site on demand, terminated by a `rustls` reverse proxy. No OpenSSL, no `mkcert`. Once you trust the CA (one-time), every `.test` site is green-padlock valid.

```sh
yerd secure my-app         # https://my-app.test  (trusted via the local CA)
yerd unsecure my-app       # back to plain HTTP
```

::: info Trust is one-time
HTTPS becomes trusted system-wide after `sudo yerd elevate trust` adds the CA to your system trust store. See [Elevation & Privileges](./elevation).
:::

[HTTPS & certificates guide →](./https)

## Local `.test` DNS

An embedded resolver (built on `hickory-dns`) answers `*.test` lookups and points them at the daemon's reverse proxy. After a one-time setup step, `http://blog.test` just works with no per-site `/etc/hosts` editing.

```sh
sudo yerd elevate resolver   # one-time: route *.test to Yerd's resolver
```

[DNS & .test domains guide →](./dns)

## Rootless operation & elevation

The daemon and GUI run as your user, never as root. Privileged effects are confined to `yerd-helper`, which takes typed arguments, never shells out, never touches the network, does one thing, and exits. Setup may elevate once; daily use never does.

```sh
sudo yerd elevate            # trust the CA, route *.test, allow ports 80/443
sudo yerd elevate trust      # ...or run just one piece
sudo yerd elevate resolver
sudo yerd elevate ports
yerd unelevate               # reverse what elevate configured
```

::: tip Prefer no sudo at all?
Skip elevation and run sites on `127.0.0.1:8080` / `:8443`. Yerd binds those unprivileged ports out of the box and falls back to them when it can't bind 80/443. Can't even route `.test`? Reach any site through `http://localhost:8080/~<name>.test` - see [Localhost Access (No Resolver)](./localhost-access).
:::

[Elevation & privileges guide →](./elevation)

## The background daemon

`yerdd` is one lightweight (~8 MB) native binary that owns all runtime state and serves the reverse proxy, DNS resolver, PHP-FPM pools, and database/cache services. No VM, no container, no Electron. It's bundled inside the app; on Linux it runs as a `systemd --user` service, and the GUI starts it for you.

```sh
systemctl --user enable --now yerd   # Linux, runs as you
yerdd serve &                        # from source
yerd restart daemon                  # restart via the CLI
```

[Daemon guide →](./daemon)

## Diagnostics: status & doctor

`yerd status` gives a live snapshot of daemon state, ports, DNS, CA trust, and per-version PHP pools (PID, RAM, load). `yerd doctor` checks for common problems and explains the fixes; `yerd doctor fix` auto-repairs the safe ones.

```sh
yerd status        # snapshot: daemon, ports, DNS, CA trust, PHP pools, load
yerd doctor        # diagnose common problems
yerd doctor fix    # auto-repair the safe ones
```

[Diagnostics guide →](./diagnostics)

## Desktop app

A Tauri v2 tray app (Vue 3 + TypeScript) ships as a single bundle: `.dmg` on macOS, `.deb` on Linux (plus a native Arch `.pkg.tar.zst` for x86-64). It's the recommended way to run Yerd, surfacing the same data and actions as the CLI in a native tray UI.

::: tip The app sets up the backend for you
The daemon, the `yerd` CLI, and `yerd-helper` are all **bundled inside the app** (Apple Silicon macOS · Linux x86-64 and arm64). On first launch it starts the daemon - no separate CLI install needed.
:::

[Desktop app guide →](./desktop-app)

## CLI with `--json`

The `yerd` CLI covers everything the daemon does. Output is human-readable by default; add `--json` to any command for machine-readable output. Both render from the same response, so they never drift.

```sh
yerd sites --json | jq '.[].name'
yerd status --json
yerd list php --json
```

| Command | What it does |
|---|---|
| `yerd park <dir>` | Park a directory; each child folder is served at `<name>.test`. |
| `yerd link <name> <dir>` | Serve a single directory as a named site. |
| `yerd unlink <name>` | Remove a linked / parked site. |
| `yerd sites` | List every known site (kind, PHP version, HTTPS, doc-root). |
| `yerd use <version>` | Set the global default PHP version. |
| `yerd use <site> <version>` | Set one site's PHP version. |
| `yerd secure <site>` / `unsecure <site>` | Turn HTTPS on / off for a site. |
| `yerd install php <version>` | Download and install a PHP version. |
| `yerd list php [--check]` | List installed PHP versions (and available updates). |
| `yerd update php [<version>]` | Update one (or all) installed PHP versions. |
| `yerd status` | Snapshot of the daemon, ports, DNS, CA trust, and PHP pools. |
| `yerd doctor` / `yerd doctor fix` | Diagnose common problems; auto-repair the safe ones. |
| `yerd elevate [trust\|resolver\|ports]` | One-time privileged setup (run with `sudo`). |

[Full CLI reference →](../reference/cli/)

## Developer tooling

Install the tools a PHP/Laravel project needs - Composer, Node (`node`/`npm`/`npx`),
and Bun (`bun`/`bunx`) - as self-contained binaries on your `PATH`, with no global
install or system package manager. Yerd downloads the latest release (LTS for
Node), verifies it, and drops the commands alongside its `php` shims.

```sh
yerd install tool node     # latest Node LTS — node, npm, npx
yerd install tool composer
```

[Tooling guide →](./tooling)

## Databases & caches

Yerd installs and supervises MySQL, MariaDB, PostgreSQL, and Redis (Valkey) as
native, per-user processes - no Docker. Start and stop them from the CLI or the
desktop app, and create, drop, back up, and restore SQL databases without a
separate client. Engines run on loopback with no elevation.

```sh
yerd service install redis 8
yerd db create mysql my_app
```

[Services & Databases →](./services)

## Mail capture

Yerd runs a built-in SMTP sink on `127.0.0.1:2525`, enabled by default, that
catches every message your apps try to send and keeps it locally for inspection
- it never relays anything to the outside world. Browse captured mail in the
desktop app or straight from the CLI, so testing transactional email needs no
third-party service.

```sh
yerd mail                  # list captured messages
```

[Mail capture guide →](./mail)

## Laravel Dumps

A native PHP extension streams Laravel dumps, queries, jobs, views, requests,
logs, and cache events live into a dedicated desktop-app window as they happen -
a richer, always-on view than `dd()` in the browser. It's off by default; enable
it when you want the live feed.

[Laravel Dumps guide →](./laravel-dumps)
