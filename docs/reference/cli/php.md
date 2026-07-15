# PHP

Yerd downloads prebuilt static PHP builds and runs an FPM pool per installed version. The [PHP Versions guide](../../guide/php-versions) covers this in depth.

## Choosing the version

The `use` command is overloaded by argument count:

| Command | Description | Example |
| --- | --- | --- |
| `yerd use <VERSION>` | Set the **global** default: the terminal `php` shim and the per-site fallback. | `yerd use 8.5` |
| `yerd use <SITE> <VERSION>` | Set the PHP version for a single named site. | `yerd use blog 8.3` |

```sh
yerd use 8.5          # global default for the `php` shim and new sites
yerd use blog 8.3     # pin one site to 8.3
```

After a successful global `yerd use <version>` (human output only), `yerd` prints a hint telling you which directory holds the managed `php` shim and warns if a different `php` is found earlier on your `PATH` and would shadow it.

## Managing installed versions

| Command | Description | Example |
| --- | --- | --- |
| `yerd install php <VERSION>` | Install a PHP version (downloads a prebuilt static build). | `yerd install php 8.5` |
| `yerd uninstall php <VERSION>` | Uninstall a PHP version (removes its files; blocked if in use). | `yerd uninstall php 8.3` |
| `yerd update php [VERSION]` | Update a PHP version to the latest release. Omit the version to update every installed version. | `yerd update php` |
| `yerd restart php [VERSION]` | Restart a PHP FPM pool. Omit the version to restart every running pool. | `yerd restart php 8.5` |
| `yerd list php [--check] [--available]` | List installed PHP versions and the global default. | `yerd list php` |

```sh
yerd install php 8.5      # download + run an 8.5 FPM pool
yerd update php           # update all installed versions to latest
yerd update php 8.5       # update just 8.5
yerd restart php          # restart every running pool
yerd uninstall php 8.3    # remove 8.3 (refused if a site still uses it)
```

### `yerd list php` flags

| Flag | Description |
| --- | --- |
| `--check` | Poll the distribution now to refresh "update available" status. Without it, status is served from the daemon's cache (no network). |
| `--available` | List the versions installable from the distribution instead, tagging ones already installed. **Takes precedence over `--check`.** |

```sh
yerd list php                 # installed versions, from cache (no network)
yerd list php --check         # installed versions, freshly checking for updates
yerd list php --available     # everything installable, tagging what you have
```

Installed versions are printed one per line; the current default is marked `(default)`, and any version with a newer release shows `update available: <installed> -> <latest>`. If nothing is installed, `yerd list php` suggests `yerd install php <default>`.

## Global PHP ini settings

`set` and `unset` manage global PHP ini defaults that are applied to **every** installed version. `set` writes a value; `unset` resets a setting back to PHP's built-in default (the wire convention is an empty value).

| Command | Description | Example |
| --- | --- | --- |
| `yerd set php <SETTING> <VALUE> [--php <VERSION>]` | Set a PHP ini default. With `--php`, only that installed version is affected (a per-version override). | `yerd set php memory_limit 512M` |
| `yerd unset php <SETTING> [--php <VERSION>]` | Reset a setting to PHP's built-in value. With `--php`, only that version's override is removed (the global default applies again). | `yerd unset php memory_limit` |

```sh
yerd set php memory_limit 512M
yerd set php display_errors On
yerd unset php memory_limit
yerd set php memory_limit 1G --php 8.3    # only PHP 8.3 gets 1G
yerd unset php memory_limit --php 8.3     # 8.3 inherits the global value again
```

**Precedence:** a version's effective value is its `--php` override when set, else
the global value, else PHP's built-in default. Changing a per-version value
restarts **only** that version's pool; a global change restarts every running
pool. Per-version values survive uninstalling and reinstalling the version.

The setting name (and, for `set`, the value) is validated client-side before connecting, so a typo or an out-of-shape value is a clean usage error rather than a round-trip. The supported settings are:

| Setting | Shape |
| --- | --- |
| `memory_limit` | byte size (e.g. `512M`), or `-1` for unlimited |
| `max_execution_time` | integer |
| `max_input_time` | integer |
| `max_file_uploads` | integer |
| `upload_max_filesize` | byte size (e.g. `64M`) |
| `post_max_size` | byte size (e.g. `64M`) |
| `display_errors` | boolean flag (e.g. `On` / `Off`) |
| `error_reporting` | an `error_reporting` expression |

::: tip
The configured settings are echoed back by `yerd list php` under a `settings:` block, so you can confirm what's currently applied. See the [Configuration Reference](../configuration) for how these are stored and rendered into FPM config.
:::

## Custom extensions

`yerd php ext` registers extra PHP extensions (`.so`) that Yerd's builds don't
ship. A registered extension loads into **both** the FPM (web) runtime and the CLI
for its version. Native extensions are ABI-bound to a PHP minor, so each is
registered under one version.

| Command | Description | Example |
| --- | --- | --- |
| `yerd php ext add <VERSION> <PATH> [--zend] [--name <NAME>]` | Register an extension for a version. | `yerd php ext add 8.5 /opt/php/pecl/scrypt.so` |
| `yerd php ext remove <VERSION> <NAME>` | Remove a registered extension by name. | `yerd php ext remove 8.5 scrypt` |
| `yerd php ext list` | List registered extensions, grouped by version. | `yerd php ext list` |

```sh
yerd php ext add 8.5 /opt/homebrew/lib/php/pecl/20250925/scrypt.so
yerd php ext add 8.5 /opt/php/xdebug.so --zend --name xdebug
yerd php ext list
yerd php ext remove 8.5 scrypt
```

- `VERSION` is a `major.minor` (e.g. `8.5`) and must be installed.
- `PATH` must be an absolute path ending in `.so`, with no control characters,
  NUL, `"`, or `$` (spaces are allowed). It is validated client-side before
  connecting, so a bad path is a clean usage error rather than a round-trip.
- `--zend` loads it as a `zend_extension` (xdebug/opcache-style) rather than a
  plain `extension`.
- `--name` sets the removal/display handle; it defaults to the `.so` basename.

On `add`, the daemon **load-probes** the `.so` against that version's PHP and
rejects it if it can't load (wrong-version build, missing dependency, or a Zend
extension registered without `--zend`), so a bad extension is a clear error rather
than a broken pool. `add`/`remove` restart that version's running FPM pool.
`yerd php ext list` tags any extension whose `.so` is missing on disk with
`(missing!)`. See the [Configuration Reference](../configuration#php) for how the
registry is stored.
