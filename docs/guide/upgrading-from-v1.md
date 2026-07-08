# Upgrade Guide

Coming from the original Go-based Yerd (`LumoSolutions/yerd`, "v1")? This page covers what changed and how to move your local sites to v2.

::: warning This is a replacement, not an in-place upgrade.
Yerd v2 (`forjedio/yerd`) is a ground-up Rust rewrite. Different binaries, a different on-disk layout, an incompatible config format, and a redesigned command set. There is no automatic migration. You uninstall v1 and set up v2 fresh.
:::

## What changed

| Area | v1 (Go) | v2 (Rust) |
|---|---|---|
| Privileges | leaned on `sudo` for most operations | rootless; elevates once, then runs as your user |
| PHP | built from source | prebuilt, signed static builds downloaded on demand |
| Platforms | - | macOS + Linux today (Windows on the [roadmap](./services)) |
| Config & layout | v1's format | new, incompatible TOML config and layout |
| Commands | v1's names | redesigned; don't assume names carry over |

So v2 installs small and fast (no compiler needed for PHP), doesn't ask for your password every time you touch a site, and behaves the same across operating systems.

::: info Rootless by design
`sudo` appears in only two places: installing the system package (normal for any `.deb`/`.pkg.tar.zst`), and the one-time `sudo yerd elevate`. After that, day-to-day use never touches root. See [Elevation & Privileges](./elevation).
:::

## What does not carry over

- **v1 config is not read.** v2 uses its own TOML config; see the [Configuration Reference](../reference/configuration).
- **PHP versions are not reused.** Reinstall the versions you need (fast, no compilation).
- **Sites are not migrated.** Re-park and re-link them with the v2 commands below.
- **Command names may differ.** Use the [CLI Reference](../reference/cli/) or `yerd --help`.
- **The local CA is regenerated.** You trust the new CA once during setup.

## Migration, step by step

### 1. Stop and remove the v1 install

Shut down anything v1 is running so it can't fight v2 over ports 80/443, the DNS resolver, or your `.test` domains. Stop its daemon/service, then uninstall the Go binary and its files.

If v1 changed your system DNS resolver or installed a CA into your trust store, undo those too (v1's own uninstall is the right tool). A stale resolver or trusted CA is the most common cause of "it half-works" symptoms after switching.

::: tip
After removing v1, reboot or flush DNS so the OS forgets v1's resolver before v2 installs its own.
:::

### 2. Install Yerd v2

Follow **[Getting Started](./getting-started)** to install the app and go through its first-run onboarding journey - it installs and starts the daemon for you. You can install a PHP version and park a projects folder there too, or hold off and do it explicitly in the migration-specific steps below.

### 3. Run the one-time privileged setup

The only command that uses root in normal use. It trusts the local CA, routes `*.test` to Yerd's DNS responder, and lets the daemon bind ports 80/443:

```sh
sudo yerd elevate
```

You can grant the pieces individually:

```sh
sudo yerd elevate trust       # trust the local CA in the system store
sudo yerd elevate resolver    # route *.test queries to Yerd's DNS responder
sudo yerd elevate ports       # allow the daemon to bind ports 80/443
```

Reverse any of these with `sudo yerd unelevate` (optionally `trust`, `resolver`, or `ports`). See [Elevation & Privileges](./elevation).

::: tip Ports without elevation
Skip `sudo yerd elevate ports` and the daemon falls back to `8080`/`8443`. `yerd doctor` reports what's in effect.
:::

### 4. Reinstall the PHP versions you need

These download prebuilt static builds, so it's quick:

```sh
yerd install php 8.5
yerd install php 8.3
```

Set a global default (this drives the terminal `php` shim and the per-site fallback):

```sh
yerd use 8.5
```

Check what's installed and what updates are available:

```sh
yerd list php                 # installed versions + the global default
yerd list php --available     # versions installable from the distribution
yerd list php --check         # refresh "update available" status (polls now)
```

See [PHP Versions](./php-versions) for installs, updates, and per-site pinning.

### 5. Re-park and re-link your sites

To turn a directory whose sub-folders should each become a `.test` site, **park** it:

```sh
yerd park ~/Sites
#   ~/Sites/blog  ->  http://blog.test
```

For a single project served under a name you choose, **link** it:

```sh
yerd link my-app ~/code/my-app
#   ->  http://my-app.test
```

Verify:

```sh
yerd sites                    # every parked or linked site
yerd list parked              # the registered parked directory roots
```

Later, `yerd unlink <name>` removes a linked site and `yerd unpark <path>` un-parks a directory. See [Sites](./sites).

### 6. Turn HTTPS back on per site

HTTPS isn't on by default. Promote the sites that need it:

```sh
yerd secure my-app
#   ->  https://my-app.test  (trusted, via the local CA)
```

Use `yerd unsecure <name>` to turn it off. Since the CA was trusted in step 3, secured sites get a green padlock with no browser warnings. See [HTTPS & Certificates](./https).

### 7. Pin per-site PHP versions (optional)

The two-argument form of `yerd use` targets a single site:

```sh
yerd use my-app 8.3
```

### 8. Confirm everything is healthy

```sh
yerd status                   # daemon, proxy, DNS, ports, CA, PHP health
yerd doctor                   # diagnose common problems
yerd doctor fix               # attempt safe, unprivileged repairs
```

`yerd doctor` is most useful right after migrating: it flags a left-over v1 resolver, a port conflict, or an untrusted CA and tells you what to do. Add `--json` for machine-readable output. See [Diagnostics](./diagnostics).

## Command map

Commands you'll use most while migrating. For the rest, see `yerd --help` and the [CLI Reference](../reference/cli/).

| Task | v2 command |
|---|---|
| Park a directory of projects | `yerd park <dir>` |
| Link one project as a named site | `yerd link <name> <dir>` |
| Remove a linked site | `yerd unlink <name>` |
| Un-park a directory | `yerd unpark <dir>` |
| List sites | `yerd sites` |
| List parked roots | `yerd list parked` |
| Install a PHP version | `yerd install php <version>` |
| Set the global PHP default | `yerd use <version>` |
| Pin a site's PHP version | `yerd use <site> <version>` |
| List / update PHP | `yerd list php` · `yerd update php [<version>]` |
| HTTPS on / off | `yerd secure <site>` · `yerd unsecure <site>` |
| Manage a site's domains | `yerd domain list\|add\|remove\|primary\|reset <site>` |
| One-time privileged setup | `sudo yerd elevate [trust\|resolver\|ports]` |
| Reverse setup | `sudo yerd unelevate [...]` |
| Health & repair | `yerd status` · `yerd doctor` · `yerd doctor fix` |

v2 lets a site answer multiple domains, subdomains, and wildcards through `yerd domain` (see the [domains reference](../reference/cli/domains)). Unlike some setups, subdomains are explicit: a site answers only its exact apex until you add more.

::: details Lineage
Yerd v2 is a ground-up rewrite of our own v1 package ([`LumoSolutions/yerd`](https://github.com/LumoSolutions/yerd)). v1 is reference-only: no command-surface or config-format compatibility. The full project lives at [github.com/forjedio/yerd](https://github.com/forjedio/yerd).
:::

## Where to go next

- New to the concepts? See the [Introduction](./introduction) and [Features](./desktop-app).
- Setting up from scratch: [Getting Started](./getting-started).
- The full command list: [CLI Reference](../reference/cli/).
- The new config format and layout: [Configuration Reference](../reference/configuration).
