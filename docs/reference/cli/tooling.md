# Tooling

Yerd installs developer tools - **Composer**, **Node.js** (`node`/`npm`/`npx`),
**Bun** (`bun`/`bunx`), **pnpm** (`pnpm`/`pnpx`), the **Laravel installer** (`laravel`), and **WP-CLI**
(`wp`) - as self-contained binaries on your `PATH`. Each is identified by a
short `id`: `composer`, `node`, `bun`, `pnpm`, `laravel`, or `wp-cli`. The
[Tooling guide](../../guide/tooling) covers the model in depth; this page is the
command reference.

::: info Managed versions
Node releases are retained side by side. Other tools install their latest release.
Installing your first tool from the CLI automatically adds Yerd's bin directory
to your `PATH`; you can also run [`yerd path install`](#path-setup).
:::

## Listing

| Command | Description |
| --- | --- |
| `yerd tools` | List every tool: install status, installed version, and the commands it provides. |

```sh
yerd tools
```

```text
TOOL      STATUS          COMMANDS       LOCATION
composer  2.10.1          composer       -
node      external        node,npm,npx   /opt/homebrew/bin/node
bun       not installed   bun,bunx       -
pnpm      not installed   pnpm,pnpx      -
```

`LOCATION` is only populated for `external` tools - ones already on your
`PATH` from somewhere other than Yerd (Homebrew, `nvm`/`fnm`, a global
Composer, …). See the [Tooling guide](../../guide/tooling#external-tools) for
what that means and why there's no install/update action for them.

Add `--json` for machine-readable output.

## Installing & updating

| Command | Description | Example |
| --- | --- | --- |
| `yerd install tool <ID>` | Install the tool's latest version, then expose its commands on `PATH` - a **verified release download** for `node` / `bun` / `pnpm` / `composer`, or a **Composer build** (`create-project`) for `laravel` / `wp-cli`. **Idempotent** - run again to update to the current latest. For `node` the new release is installed beside the old one and an existing default is kept, so select it with `yerd node use <version>` to move the `node`/`npm`/`npx` shims. | `yerd install tool node` |
| `yerd uninstall tool <ID>` | Remove the tool's files and its `PATH` commands. | `yerd uninstall tool bun` |

```sh
yerd install tool composer    # PHP dependency manager (needs a PHP version)
yerd install tool node        # latest Node LTS - node, npm, npx
yerd install tool bun         # bun + bunx
yerd install tool pnpm        # pnpm + pnpx
yerd install tool laravel     # the laravel new installer (needs Composer)
yerd install tool wp-cli      # the wp command for WordPress (needs Composer)
yerd install tool node        # run again to add the newest LTS beside the old one
yerd node use 24              # then select it; the default isn't moved for you
yerd uninstall tool bun       # remove bun and prune its shims
```

`<ID>` is one of `composer`, `node`, `bun`, `pnpm`, `laravel`, or `wp-cli`. An unknown
id returns a `not_found` error.

::: warning Composer requires PHP
`composer` runs under Yerd's managed PHP, so install at least one
[PHP version](./php) first. Node, Bun, and pnpm are standalone. The Laravel installer
and WP-CLI are Composer packages, so they also need Yerd's own Composer
installed first.
:::

::: tip WP-CLI has no phar self-update
Yerd's `wp-cli` is a Composer install, so WP-CLI's own `wp cli update`
subcommand isn't applicable and will error - run `yerd install tool wp-cli`
again instead to update.
:::

## Node versions

```sh
yerd install tool node 24       # latest available 24.x release
yerd install tool node v22.9.0  # exact release
yerd install tool node lts/iron # newest release of the Iron LTS line
yerd node list                  # installed releases and global default
yerd node use 24                # select highest installed 24.x globally
yerd node use 22 --site blog    # save a preference for blog
yerd node unset --site blog     # clear blog's saved preference
yerd node uninstall 22.9.0      # remove one installed release
yerd exec npm install           # use the project's selected Node and npm
yerd which --json node          # show version, path, site, and source
```

| Command | Description |
| --- | --- |
| `yerd node list` | Installed releases, marking the one the global default resolves to with `(default)` (a partial default such as `22` marks its highest installed 22.x). |
| `yerd node use <SELECTOR> [--site <NAME>]` | Resolve the selector against installed releases and save the exact version as the global default or the site's preference. |
| `yerd node unset --site <NAME>` | Clear a site's saved preference, so it follows its project file or the global default. Idempotent. |
| `yerd node uninstall <VERSION>` | Remove one exact installed release and print the releases left. Refused while the global default resolves only to it; site preferences that would dangle are cleared. |

Numeric selectors accept a major, major.minor, or exact major.minor.patch, with
an optional `v` prefix. Partial selectors choose the highest matching version.
`node` selects the newest release, `lts/*` the newest LTS release, and
`lts/<codename>` (for example `lts/iron`, any case) the newest release of that LTS
line. Quote `'lts/*'` in zsh. Install resolves against Node's upstream release
index; `node use`, `exec`, and `which` resolve only against complete installed
releases, matching `lts/<codename>` against the codename recorded at install. No
runtime is downloaded when executing a command.

`yerd install tool node [<SELECTOR>]` streams its progress like other tool
installs (with `--json` it blocks and prints the result). A versioned install
preserves an existing global default. The first installation becomes the default
if none exists. `yerd install tool node` installs the newest LTS, retaining
previously installed releases; it becomes the default only when there is none, so
an existing default is kept. Releases below major 9 are not installable, including
`yerd install tool node 8`. In the desktop app, the [Node page](../../guide/node)
browses, installs, updates, and uninstalls individual releases.

`yerd exec node`, `npm`, and `npx` select the nearest project `.nvmrc` or
`.node-version`, then a saved site preference, then the global default. See
[Node resolution](./exec#node-resolution). Bare `node`, `npm`, and `npx` commands
on Yerd's `PATH` continue to use the global default. `yerd tools` shows that
default's version; the desktop app's Node page stars it.

`yerd uninstall tool node` removes all managed Node releases and their commands,
and clears the global default and every saved site preference. Existing installations
in `{data}/tools/node` remain usable without migration. New releases live in
`{data}/tools/node-versions/<version>`.

## PATH setup

The tool commands live in Yerd's `{data}/bin` directory. Manage your shell's
`PATH` entry for it with `yerd path`:

| Command | Description |
| --- | --- |
| `yerd path install` | Add `{data}/bin` to your shell startup file (idempotent; covers zsh, bash, and fish). |
| `yerd path uninstall` | Remove the Yerd `PATH` block from your shell startup file. |
| `yerd path print` | Print the shell snippet without modifying any file (for `eval` / manual use). |

```sh
yerd path install     # then open a new terminal
```

## Exit codes

These commands follow the standard CLI [exit codes](./#exit-codes): `0` on
success, `1` on a daemon error (e.g. an unknown tool id, a failed download, or a
checksum mismatch), and `69` if the daemon is unreachable.

## See also

- [Tooling guide](../../guide/tooling) - the full model and where files live.
- [PHP reference](./php) - the version model these tools follow.
- [Services reference](./services) - the same install-on-demand approach.
