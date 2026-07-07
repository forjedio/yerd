# Tooling

Yerd installs developer tools - **Composer**, **Node.js** (`node`/`npm`/`npx`),
**Bun** (`bun`/`bunx`), the **Laravel installer** (`laravel`), and **WP-CLI**
(`wp`) - as self-contained binaries on your `PATH`. Each is identified by a
short `id`: `composer`, `node`, `bun`, `laravel`, or `wp-cli`. The
[Tooling guide](../../guide/tooling) covers the model in depth; this page is the
command reference.

::: info Latest only
Yerd installs the latest stable release of each tool (latest **LTS** for Node).
There is no per-version selection - installing again updates to the current
latest. Installing your first tool from the CLI **automatically adds** Yerd's bin
directory to your `PATH`; you can also manage it yourself with
[`yerd path install`](#path-setup). If the bin directory isn't on your `PATH`,
[`yerd doctor`](./diagnostics) flags it with the one-line fix.
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
```

`LOCATION` is only populated for `external` tools - ones already on your
`PATH` from somewhere other than Yerd (Homebrew, `nvm`/`fnm`, a global
Composer, …). See the [Tooling guide](../../guide/tooling#external-tools) for
what that means and why there's no install/update action for them.

Add `--json` for machine-readable output.

## Installing & updating

| Command | Description | Example |
| --- | --- | --- |
| `yerd install tool <ID>` | Install the tool's latest version, then expose its commands on `PATH` - a **verified release download** for `node` / `bun` / `composer`, or a **Composer build** (`create-project`) for `laravel` / `wp-cli`. **Idempotent** - run again to update to the current latest. | `yerd install tool node` |
| `yerd uninstall tool <ID>` | Remove the tool's files and its `PATH` commands. | `yerd uninstall tool bun` |

```sh
yerd install tool composer    # PHP dependency manager (needs a PHP version)
yerd install tool node        # latest Node LTS - node, npm, npx
yerd install tool bun         # bun + bunx
yerd install tool laravel     # the laravel new installer (needs Composer)
yerd install tool wp-cli      # the wp command for WordPress (needs Composer)
yerd install tool node        # run again to update to the newest LTS
yerd uninstall tool bun       # remove bun and prune its shims
```

`<ID>` is one of `composer`, `node`, `bun`, `laravel`, or `wp-cli`. An unknown
id returns a `not_found` error.

::: warning Composer requires PHP
`composer` runs under Yerd's managed PHP, so install at least one
[PHP version](./php) first. Node and Bun are standalone. The Laravel installer
and WP-CLI are Composer packages, so they also need Yerd's own Composer
installed first.
:::

::: tip WP-CLI has no phar self-update
Yerd's `wp-cli` is a Composer install, so WP-CLI's own `wp cli update`
subcommand isn't applicable and will error - run `yerd install tool wp-cli`
again instead to update.
:::

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
