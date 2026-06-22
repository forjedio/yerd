# Tooling

Yerd can install the **developer tools** a typical PHP/Laravel project reaches
for — [Composer](https://getcomposer.org), [Node.js](https://nodejs.org) (with
`npm`/`npx`), and [Bun](https://bun.sh) — the same way it installs
[PHP versions](./php-versions): prebuilt, self-contained binaries downloaded on
demand and dropped onto your `PATH`. No system package manager, no global
install, nothing to uninstall by hand.

| Tool | `id` | Provides | Source |
|---|---|---|---|
| Composer | `composer` | `composer` | getcomposer.org (phar) |
| Node.js | `node` | `node`, `npm`, `npx` | nodejs.org (latest LTS) |
| Bun | `bun` | `bun`, `bunx` | github.com/oven-sh/bun |

::: tip Why bundle these?
A fresh machine that has Yerd shouldn't also need Homebrew, `nvm`, or a global
Composer just to run a Laravel app with a Vite front-end. Yerd keeps these tools
in its own data directory, isolated from anything else on your system, and
removes them cleanly on uninstall.
:::

## In the desktop app

Open the **Tooling** page from the sidebar (under the **Developer** group). It
lists the developer tools Yerd manages and their install status:

<ThemedImage light="/images/tooling-light.png" dark="/images/tooling-dark.png" alt="The Tooling page in the Yerd desktop app" />

- **Composer**, **Node**, and **Bun**, each showing the commands it provides.
- Click **Install** to fetch the latest release; once installed you get
  **Update** (re-fetch the current latest) and **Uninstall**.
- Each tool is placed on your `PATH` alongside PHP and managed entirely by Yerd,
  so it won't collide with a system install.

## From the command line

```sh
yerd tools                      # list the tools and their install status
yerd install tool node          # download + install the latest Node LTS
yerd install tool bun
yerd install tool composer
yerd uninstall tool bun         # remove a tool and its PATH commands
```

`yerd install tool <id>` is idempotent — run it again to update to the current
latest. See the [Tooling CLI reference](../reference/cli/tooling) for the exact
command surface.

## How it works

The model mirrors [PHP versions](./php-versions) and [services](./services):

- **Self-contained binaries.** Each tool is a relocatable build — Node's tarball
  bundles `node` + `npm` + `npx`, Bun is a single binary, Composer is a phar run
  by Yerd's managed PHP. Nothing is compiled and nothing touches system paths.
- **Verified downloads.** Every artifact is checked against the publisher's
  `SHASUMS256.txt` (Node, Bun) or `composer.phar.sha256sum` (Composer) before it
  is installed.
- **Installed under Yerd's data dir.** Tools live in `{data}/tools/<id>/`
  (e.g. `~/Library/Application Support/io.yerd.Yerd/tools` on macOS), a sibling of
  your PHP installs — so a PHP update never disturbs them.
- **Exposed on `PATH`.** Their commands are symlinked into `{data}/bin`, the same
  directory that holds the `php`/`php<ver>` shims. Put that directory on your
  `PATH` once (see below) and `composer`, `node`, `npm`, `bun`, … just work.
- **Rootless.** Everything runs as your user, no elevation.

### Latest only

Yerd installs the **latest stable** release of each tool (the latest **LTS** for
Node). There is no per-project version picker — **Update** simply re-fetches the
current latest and replaces it in place. If you need to pin a specific Node
version per project, a system version manager like `nvm`/`fnm` is still the right
tool; Yerd's goal here is a good default that's always there.

## Put Yerd's bin directory on your PATH

The tool commands live in Yerd's `{data}/bin` directory. Add it to your shell
once:

```sh
yerd path install     # adds {data}/bin to your shell startup file
```

Open a new terminal afterwards (or `source` your shell file). Then:

```sh
which composer        # → …/io.yerd.Yerd/bin/composer
node --version
npm --version
bun --version
```

`yerd path install` writes a small, guarded block to your shell's startup file
(`.zshrc`, `.bashrc`/`.bash_profile`, or `config.fish`). `yerd path uninstall`
removes it; `yerd path print` shows the snippet without touching any file.

::: info Coexisting with Herd, Homebrew, or nvm
Yerd's `bin` directory is **prepended** to `PATH`, so its `node`/`composer` take
precedence over other copies on your machine. If you'd rather your existing
tools win, put their directories earlier in your shell file. Nothing Yerd
installs ever shadows a tool you didn't ask it to manage.
:::

## Composer needs PHP

Composer is a phar, so it runs under Yerd's managed PHP — `composer` resolves to
your [default PHP version](./php-versions). Install at least one PHP version
first (`yerd install php 8.4`); otherwise `composer` reports that no PHP is
available. Node and Bun are standalone and have no such dependency.

::: tip ext-intl and friends
Yerd's PHP builds ship the **bulk** static-php-cli extension set, including
`intl`, `sodium`, `mysqli`, and more — so Composer packages that require them
install without extra steps. See [PHP Versions](./php-versions) for the bundled
extension list.
:::

## Where things live

| Path | Contents |
|---|---|
| `{data}/tools/composer/composer.phar` | The Composer phar. |
| `{data}/tools/node/node-<ver>-<os>-<arch>/` | The unpacked Node distribution. |
| `{data}/tools/bun/bun-<os>-<arch>/bun` | The Bun binary. |
| `{data}/bin/{composer,node,npm,npx,bun,bunx}` | The `PATH` shims. |

`{data}` is Yerd's per-user data directory (`yerd status` and
`yerd path print` both show the exact path for your platform).

## See also

- [Tooling CLI reference](../reference/cli/tooling) — every command and flag.
- [PHP Versions](./php-versions) — the version model these tools follow.
- [Services & Databases](./services) — the same install-on-demand approach for
  databases and caches.
