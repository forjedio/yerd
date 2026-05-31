# PHP Versions

Yerd runs **any number of PHP versions side by side** and lets you pick which one each site uses. PHP isn't bundled, so the install stays small. The first time you ask for a version, Yerd downloads a prebuilt static [`static-php-cli`](https://github.com/crazywhalecc/static-php-cli) build and supervises one PHP-FPM pool per version behind the [reverse proxy](./sites).

## Installing a version

```sh
yerd install php 8.5
```

Yerd detects your platform (`linux`/`macos`, `x86_64`/`aarch64`), fetches the live listing from the static-php-cli distribution, resolves the latest published patch of that minor, downloads the CLI and FPM tarballs, then atomically swaps them into place. Versions are discovered from the distribution at runtime, so a brand-new PHP patch is installable the day it ships, with no Yerd release needed.

Installs are **idempotent**: running it again replaces the directory with a fresh download of the latest patch. If the version isn't published for your platform, the install fails cleanly and writes nothing. The running daemon picks up a new version automatically, no restart required.

::: info A version is always a major.minor
A "PHP version" means a `major.minor` pair like `8.5`, never a full patch like `8.3.12`. Yerd installs and tracks the latest patch of the minor you ask for, and updates move you to a newer patch of that same minor. Input is `8.5` (or `php8.5`); major must be `5..=9`, minor `0..=99`.
:::

::: info Integrity is TLS-pinned, not hash-pinned
The distribution publishes no checksums, so Yerd verifies downloads over HTTPS to the distribution host rather than a pinned SHA-256. That keeps the supported version set from being frozen into the binary. (Yerd's own release artifacts are separately verified against a `SHA256SUMS` manifest; see [Getting Started](./getting-started).)
:::

## How versions are stored

Each install lands under the per-user data directory:

```text
{data}/php/php-8.5/bin/php          # the CLI interpreter
{data}/php/php-8.5/sbin/php-fpm     # the FastCGI process manager
{data}/php/php-8.5/.yerd-version    # the exact patch installed, e.g. "8.5.6"
{data}/bin/php                      # symlink → the default version's CLI
```

The dir is named for the **major.minor** (`php-8.5`); `.yerd-version` records the exact patch (`8.5.6`). Update checks read that marker to decide whether a newer patch exists. The daemon discovers installed versions by walking this directory and finding each `sbin/php-fpm` at startup.

## The global default

Yerd has one **global default** version, used for the `php` shim at `{data}/bin/php` and as the fallback for any site that hasn't pinned its own. Set it with one argument:

```sh
yerd install php 8.5
yerd use 8.5
```

A fresh config defaults to **PHP 8.3**, but you'll usually set your own right after installing.

::: tip Add the shim dir to your PATH
Put `{data}/bin` (Yerd prints the exact path) on your `PATH` so a bare `php` matches the version your sites run. The shim is a symlink, atomically re-pointed each time you change the default.
:::

## Per-site versions

Any site can pin its own version. Pass `yerd use` two arguments, a site name and a version:

```sh
yerd use my-app 8.3
```

Now `my-app.test` runs on 8.3 while every other site follows the global default.

| Site setting | Effective version |
|---|---|
| Pinned (`yerd use <site> 8.3`) | `8.3` |
| Not pinned | the global default |

Clearing a pin reverts the site to whatever the global default is at the time.

Check what each site resolves to with `yerd sites`, which lists every site with its kind, PHP version, HTTPS state, and document root. See [Sites](./sites) for parking and linking.

::: warning Pin a version you've installed
Pinning a site (or the default) to an uninstalled version means there's no FPM binary to start when a request arrives. Install it first (`yerd install php 8.3`), then pin. `yerd doctor` flags a pool that can't start.
:::

## Listing versions

```sh
yerd list php
```

This shows every installed version, marks the default, and flags any with a newer patch available. Update flags come from the **daemon's cache** by default, so no network call is made and the command is instant.

| Command | What you get |
|---|---|
| `yerd list php` | Installed versions, default, cached update flags (no network) |
| `yerd list php --check` | Same, but polls the distribution now to refresh update flags |
| `yerd list php --available` | Versions installable from the distribution, tagging installed ones |

`--available` takes precedence over `--check`. Add `--json` (a global flag) for machine-readable output.

## Updates are notify-only

Yerd checks for newer **patches** of the minors you have and tells you about them, but never installs on its own. The daemon periodically polls the distribution, compares each installed minor's latest patch against its `.yerd-version` marker, and on a newer patch logs:

```text
a newer PHP patch is available (run `yerd update php`)
```

It records this in the cache `yerd list php` reads. The poll is failure-tolerant: a network or platform failure is logged quietly and your cached state is left untouched.

Update on your terms:

```sh
yerd update php 8.5     # update just 8.5 to its latest patch
yerd update php         # update every installed version
```

An update is the same atomic install flow: it moves `8.5.4` → `8.5.6` and never jumps to a different minor. To move minors, run `yerd install php 8.6` and `yerd use 8.6` explicitly.

::: tip Nothing updates behind your back
Updates are strictly notify-only. The only automatic network call is the lightweight update check, which downloads nothing but a directory listing. Yerd downloads or swaps a PHP version only when you run `yerd update php`.
:::

## Command summary

| Command | What it does |
|---|---|
| `yerd install php <version>` | Download + install the latest patch of a minor. |
| `yerd use <version>` | Set the global default version (and the `php` shim). |
| `yerd use <site> <version>` | Pin one site to a version. |
| `yerd list php [--check]` | List installed versions; `--check` refreshes update flags. |
| `yerd list php --available` | List versions installable from the distribution. |
| `yerd update php [<version>]` | Update one (or all) versions to the latest patch. |
| `yerd uninstall php <version>` | Remove a version's files (blocked if a site uses it). |
| `yerd restart php [<version>]` | Restart one (or all) running FPM pools. |

Add `--json` to any command for machine-readable output.

## Related

- [Sites](./sites) - parking, linking, and how a request reaches an FPM pool.
- [HTTPS & Certificates](./https) - trusted HTTPS per site.
- [Diagnostics](./diagnostics) - `yerd status` and `yerd doctor` for when a pool won't start.
- [CLI Reference](../reference/cli/) - every command and flag.
- [Configuration Reference](../reference/configuration) - where the default and per-site pins live on disk.
- [yerd-php crate](../developer/crates/yerd-php) - the supervisor, version resolution, and download internals.
