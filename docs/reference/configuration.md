# Configuration Reference

Yerd stores all of its persistent state in a single TOML file: `yerd.toml`. This page documents where that file lives, every field in the schema, the defaults, how schema versioning and migration work, and how saves stay safe. Everything here is grounded in the [`yerd-config`](../developer/crates/yerd-config) crate.

::: tip You rarely edit this by hand
The daemon (`yerdd`) owns `yerd.toml`. Day to day you change it through the [CLI](./cli/) or the [desktop app](../guide/desktop-app), and the daemon rewrites the file atomically. Hand-editing works too - Yerd parses and re-validates the file on every load - but the CLI is the safer path.
:::

## Where the config file lives

The file is always named `yerd.toml` and sits in your per-OS, user-owned config directory:

| OS    | Config directory                          | Full path                                              |
| ----- | ----------------------------------------- | ------------------------------------------------------ |
| macOS | `~/Library/Application Support/io.yerd.Yerd` | `~/Library/Application Support/io.yerd.Yerd/yerd.toml` |
| Linux | `$XDG_CONFIG_HOME/yerd` (default `~/.config/yerd`) | `~/.config/yerd/yerd.toml`                       |

These paths come from [`yerd-platform`](../developer/crates/yerd-platform)'s directory resolver, which uses the `directories` crate with the qualifier `io` / `yerd` / `Yerd`. The directory is created on demand the first time the daemon saves; it is not guaranteed to exist before then.

The daemon resolves the path once at startup and falls back to `<config dir>/yerd.toml` unless an explicit path was passed on the `yerdd serve` command line. If the file is absent, the daemon starts from the built-in defaults and writes the file on the first change.

::: info Config vs. data vs. runtime
`yerd.toml` is the only file in the *config* directory. Certificates live in the *data* directory, logs in the *cache* directory, and the IPC socket in the *runtime* directory. See [Architecture](../developer/architecture) and [The Daemon](../guide/daemon) for the full layout.
:::

## Top-level schema

Every field below maps one-to-one to a field in `schema.rs`. The on-disk shape always begins with the `version` line, followed by the scalar keys, then the sub-tables.

| Key         | TOML type            | Meaning                                                            | Default        |
| ----------- | -------------------- | ----------------------------------------------------------------- | -------------- |
| `version`   | integer              | On-disk schema version. **Mandatory.**                            | `5`            |
| `tld`       | string               | TLD served by Yerd's resolver.                                    | `"test"`       |
| `dns_port`  | integer (u16)        | Loopback port for the embedded `.test` DNS responder.             | `1053`         |
| `ports`     | table                | HTTP / HTTPS listen ports.                                        | `80` / `443`   |
| `php`       | table                | PHP defaults and global ini settings.                             | see below      |
| `parked`    | table                | Parked directory paths.                                           | empty          |
| `linked`    | array of tables      | Explicitly linked sites.                                          | empty          |
| `overrides` | array of tables      | Per-site overrides for **parked** sites.                          | empty          |
| `services`  | table                | Per-service `[services.<id>]` tables; every installed engine auto-starts on boot. | empty          |
| `mail`      | table                | Built-in mail-capture SMTP server.                                | on / `2525`    |
| `dumps`     | table                | Laravel ▸ Dumps telemetry settings.                               | off / `2304`   |

::: warning Unknown keys are rejected
The parser uses `deny_unknown_fields` at every level. A typo'd or stray key (top-level, or inside `[ports]`, `[php]`, `[parked]`, `[mail]`, `[dumps]`, `[dumps.features]`, a `[services.<id>]` table, a `[[linked]]` entry, an `[[overrides]]` entry, or a `[[php.extensions.<version>]]` entry) is a hard parse error - the daemon will refuse to load the file rather than silently ignore it.
:::

### `version`

The schema version. This key is **required** - a missing `version` is a hard error (`MissingVersion`), and a non-integer or negative value is rejected (`NonIntegerVersion`). The current schema version is `10`, and Yerd always writes `version = 10`. Older `version = 1` through `version = 9` files are migrated forward automatically on load. See [Schema versioning](#schema-versioning-and-migration) below.

### `tld`

The top-level domain Yerd's resolver answers for, without a leading dot. The default is `test`, giving you `myapp.test`. The value is validated by `yerd-core`: whitespace is rejected, and a trailing dot is silently stripped (`"test."` becomes `"test"`). See [DNS & .test Domains](../guide/dns).

### `dns_port`

The loopback UDP/TCP port the embedded `.test` DNS responder binds to. The default is `1053`. A fixed (non-ephemeral) port keeps the resolver configuration installed by `yerd elevate resolver` valid across daemon restarts. A value of `0` means "ephemeral" and is intended for development and tests only - it is not durable across restarts.

::: tip Port already in use?
If another process holds `dns_port`, the daemon fails to bind and tells you to change `dns_port` in `yerd.toml` or free the port.
:::

### `[ports]`

The HTTP and HTTPS listen ports for the proxy, plus the rootless ports the daemon falls back to when it can't bind the privileged ones.

| Key              | TOML type     | Meaning                                                                          | Default |
| ---------------- | ------------- | --------------------------------------------------------------------------------- | ------- |
| `http`           | integer (u16) | HTTP listen port.                                                                | `80`    |
| `https`          | integer (u16) | HTTPS listen port.                                                               | `443`   |
| `fallback_http`  | integer (u16) | Rootless HTTP port the daemon drops to when `http` can't bind without elevation. | `8080`  |
| `fallback_https` | integer (u16) | Rootless HTTPS port the daemon drops to when `https` can't bind without elevation. | `8443`  |

The default is the IANA well-known pair `80 / 443`. Binding these privileged ports may require elevation on macOS and Linux - see [Elevation & Privileges](../guide/elevation). If you would rather avoid elevation, switch to the unprivileged fallback pair `8080 / 8443`:

```toml
[ports]
http = 8080
https = 8443
```

`fallback_http` and `fallback_https` are what the daemon binds instead of `http`/`https` when it starts in degraded mode - unable to acquire the privileged ports without elevation - so the proxy still comes up rather than failing to start. They're editable from the desktop app's Settings > Web ports card as well as by hand.

Validation rules (enforced by `Config::validate`): neither `http` nor `https` may be `0`, and they must differ (`HttpPortZero`, `HttpsPortZero`, `HttpHttpsPortsEqual`). Both fallback ports must be `>= 1024` - the fallback exists specifically to avoid needing elevation, so a privileged fallback is rejected (`FallbackPortPrivileged`) - and `fallback_http`/`fallback_https` must differ from each other (`FallbackPortsEqual`).

### `[php]`

PHP defaults applied across sites.

| Key          | TOML type | Meaning                                                      | Default |
| ------------ | --------- | ------------------------------------------------------------ | ------- |
| `default`    | string    | Default PHP version for new sites (e.g. `"8.3"`).            | `"8.3"` |
| `settings`   | table     | Global PHP ini directives applied to every installed version's FPM pool. | empty   |
| `extensions` | table     | Custom `.so` extensions to load, keyed by PHP version.       | empty   |

`default` is a `MAJOR.MINOR` version string validated by `yerd-core`'s `PhpVersion`; an out-of-range minor or a non-numeric value is rejected. See [PHP Versions](../guide/php-versions).

`[php.settings]` is a string-to-string map of PHP ini directives written into **every** installed version's FPM pool. An empty map means "use PHP's defaults" and the table is omitted from the file entirely. Only an allowlisted set of directives is accepted, and every value is validated as a security boundary (no control characters, none of the FPM/ini metacharacters `[ ] = ; #`, length ≤ 256 bytes). The supported directives are:

| Directive             | Value shape                                                   |
| --------------------- | ------------------------------------------------------------- |
| `memory_limit`        | byte size (`512M`); also accepts `-1` for unlimited           |
| `max_execution_time`  | non-negative integer                                          |
| `max_input_time`      | non-negative integer                                          |
| `max_file_uploads`    | non-negative integer                                          |
| `upload_max_filesize` | byte size (`64M`)                                             |
| `post_max_size`       | byte size (`64M`)                                             |
| `display_errors`      | boolean flag (`On` / `Off`, rendered as a `php_flag`)         |
| `error_reporting`     | integer or constant expression (e.g. `E_ALL & ~E_DEPRECATED`) |

```toml
[php.settings]
memory_limit = "512M"
max_execution_time = "300"
upload_max_filesize = "64M"
```

::: warning Setting an unsupported directive fails the load
An unknown directive name or a malformed value makes the whole config invalid (`InvalidPhpSetting`). Stick to the table above.
:::

`[php.extensions]` maps a **PHP version string** to an array of custom extensions to load into both that version's FPM pool and its CLI. It is written as an array-of-tables per version and omitted entirely when empty. Because a native `.so` is ABI-bound to a PHP minor, an entry only applies to the version it is keyed under.

| Field  | TOML type | Meaning                                                                 |
| ------ | --------- | ----------------------------------------------------------------------- |
| `name` | string    | Removal/display handle (defaults to the `.so` basename when added).     |
| `path` | string    | Absolute path to the `.so`. Validated: absolute, `.so`, no ini/shell-unsafe characters (control chars, `"`, `$`, `[ ] = ; #`, whitespace-injection). |
| `zend` | bool      | Load as a `zend_extension` rather than a plain `extension`.             |

```toml
[[php.extensions."8.5"]]
name = "scrypt"
path = "/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
zend = false
```

Manage this with [`yerd php ext`](cli/php#custom-extensions) or the desktop app's **Custom extensions** card rather than editing by hand - the CLI/daemon **load-probe** each `.so` before saving. Names must be unique within a version; a duplicate or an invalid path makes the whole config invalid.

### `[parked]`

Directories you have "parked" - every immediate subdirectory becomes a site served under `<dirname>.<tld>`. See [Sites](../guide/sites).

| Key     | TOML type        | Meaning                              | Default |
| ------- | ---------------- | ------------------------------------ | ------- |
| `paths` | array of strings | Parked directory paths.              | `[]`    |

Paths are stored **verbatim** as UTF-8 strings and are **not canonicalised** by the config layer - `"/srv/foo"` and `"/srv/foo/"` are distinct entries. They are kept in sorted order with no duplicates. An empty-string path is rejected (`ParkedPathEmpty`).

```toml
[parked]
paths = ["/Users/you/Sites", "/Users/you/work"]
```

### `[[linked]]`

Explicitly registered sites, each as its own array-of-tables entry. Order is preserved on round-trip.

| Key             | TOML type | Meaning                                            |
| --------------- | --------- | -------------------------------------------------- |
| `name`          | string    | Site name (the subdomain under your TLD).          |
| `document_root` | string    | Path to the site's project directory.              |
| `web_subpath`   | string    | Served web root, relative to `document_root`. Optional. |
| `php`           | string    | PHP version for this site (e.g. `"8.3"`).          |
| `secure`        | boolean   | Whether HTTPS is enabled for this site.            |
| `kind`          | string    | `"linked"` or `"parked"`.                          |

`name`, `document_root`, `php`, `secure`, and `kind` are required per entry. `name`, `php`, and `kind` are validated by `yerd-core`; for example an invalid site name like `"FOO.BAR"` is rejected. Linked site names must be unique - a duplicate produces `DuplicateLinkedSite`.

`web_subpath` is the directory actually served, relative to `document_root` (e.g. `"public"` for Laravel; empty/absent means "serve the document root itself"). It is **optional and omitted from the file when empty**, so a site served from its project root has no `web_subpath` line. It must be a plain relative path - an absolute path or one containing `..` is rejected (`WebRootEscapes`) so a hand-edited value can never escape the project. Yerd normally sets this for you via framework detection; see [Web root](../guide/sites#web-root-the-served-directory).

```toml
[[linked]]
name = "api"
document_root = "/Users/you/projects/api"
web_subpath = "public"
php = "8.3"
secure = true
kind = "linked"
```

### `[[overrides]]`

Per-site overrides for **parked** sites, each its own array-of-tables entry. A parked site is otherwise derived purely from a directory listing, so it has nowhere to persist a custom PHP version or HTTPS flag. Rather than promoting it to a linked site (which would change its kind), the daemon records the override here and re-applies it during the directory scan, leaving the site parked.

| Key        | TOML type | Meaning                                                       |
| ---------- | --------- | ------------------------------------------------------------- |
| `path`     | string    | The parked site's document-root path. **Required.**          |
| `php`      | string    | Pinned PHP version. Omit to inherit the global default.       |
| `secure`   | boolean   | Pinned HTTPS flag. Omit to inherit (off).                     |
| `web_root` | string    | Pinned web root, relative to `path`. Omit to auto-detect.     |

`php`, `secure`, and `web_root` are all optional - omitting a key means "inherit" (or, for `web_root`, "auto-detect on every scan"). An entry may pin one, several, or (uselessly) none. The serialiser skips omitted keys, so a partial override stays tidy on disk. Like `web_subpath` on a linked site, `web_root` must be a plain relative path inside the project (`WebRootEscapes` otherwise). Setting it is what `yerd root <parked-site> <path>` does.

```toml
# Pin PHP, HTTPS, and the served web root for one parked site...
[[overrides]]
path = "/Users/you/Sites/blog"
php = "8.4"
secure = true
web_root = "public"

# ...and only HTTPS for another (PHP and web root inherit / auto-detect).
[[overrides]]
path = "/Users/you/Sites/wiki"
secure = false
```

::: warning `path` must match byte-for-byte
The `path` key is the parked site's document-root string, stored **byte-exact and never canonicalised** - it must match exactly the path the daemon's directory scan produces. Do not canonicalise, trim, or add a trailing slash by hand, or the override won't be applied. An empty `path` is rejected (`OverridePathEmpty`).
:::

### `[services.<id>]`

Installed database / cache services, one table per engine, keyed by its `id`
(`mysql`, `mariadb`, `postgres`, or `redis`). An unknown service id fails
validation (`UnknownService`). See [Services & Databases](../guide/services).

| Key       | TOML type      | Meaning                                            | Default |
| --------- | -------------- | -------------------------------------------------- | ------- |
| `version` | string         | Installed version this engine is pinned to.        | unset   |
| `port`    | integer (u16)  | Loopback port the engine listens on.               | unset   |
| `enabled` | boolean        | Record of the last start/stop intent (status only). | `true`  |

`version` and `port` are omitted from the wire when unset; `enabled` always carries a value.

::: tip
`enabled` no longer gates boot auto-start - the daemon auto-starts **every installed** engine regardless of this flag. A `stop` lasts only the current session; `uninstall` to keep an engine off. See [Services & Databases](../guide/services#auto-start-on-boot).
:::

```toml
[services.mysql]
version = "8.4"
port = 3306
enabled = true

[services.redis]
version = "8"
port = 6379
enabled = true
```

You normally manage these through the [`yerd service`](../reference/cli/services) commands rather than by hand.

### `[mail]`

The built-in mail-capture SMTP server - a Herd-style sink that accepts mail on a loopback port and stores it for inspection in the desktop app. **Capture is on by default.**

| Key       | TOML type     | Meaning                                                | Default |
| --------- | ------------- | ------------------------------------------------------ | ------- |
| `enabled` | boolean       | Whether the daemon starts the capture server on boot.  | `true`  |
| `port`    | integer (u16) | Loopback port the capture server binds on `127.0.0.1`. | `2525`  |

When enabled the daemon binds `port` on `127.0.0.1`; a busy port is non-fatal - the daemon logs and runs with capture not listening. Validation rejects `port = 0` (`MailPortZero`).

Because the section's default (enabled, port `2525`) is the common case, the serialiser **omits `[mail]` entirely when it matches the default** - so a default file has no `[mail]` table at all. The table is written only once a value differs from the default.

```toml
[mail]
enabled = true
port = 2525
```

### `[dumps]`

Telemetry settings for the Laravel ▸ Dumps feature. The dump server buffers per-request telemetry frames from the `yerd-php-ext` extension; this section is the durable source of truth (the daemon writes a runtime mirror the extension reads each request). **Disabled by default.**

| Key       | TOML type     | Meaning                                                              | Default |
| --------- | ------------- | ------------------------------------------------------------------- | ------- |
| `enabled` | boolean       | Whether dump interception is on (the "antenna").                    | `false` |
| `port`    | integer (u16) | Loopback port the dump server listens on / the extension connects to. | `2304`  |
| `persist` | boolean       | When `false`, the buffer is cleared on each new request (latest-request view); `true` accumulates across requests. | `false` |
| `features`| table         | Per-feature capture toggles (see below).                            | empty   |

Validation rejects `port = 0` (`DumpsPortZero`).

`[dumps.features]` is a map of feature name → bool. The keys are `dumps`, `queries`, `jobs`, `views`, `requests`, `logs`, and `cache`. **An absent key means "on"**, so the table only needs entries for features you have turned *off*. An empty map (every feature on) is omitted from the file, and so is the whole `[dumps]` table when it matches the default (disabled, port `2304`, no overrides).

```toml
[dumps]
enabled = true
port = 2304
persist = false

[dumps.features]
queries = false   # absent keys default to on; only the off ones need listing
```

### `[tunnel]`

Persisted state for [sharing sites](../guide/sharing) through Cloudflare Tunnel. Two maps, both **empty by default** - the whole `[tunnel]` table is omitted from the file until you create a named tunnel or expose a site. Quick-tunnel state is never persisted (it lives only in the running daemon).

| Sub-table        | Shape                     | Meaning                                                        |
| ---------------- | ------------------------- | ------------------------------------------------------------- |
| `[tunnel.named]` | map `name → uuid`         | The named tunnels created on your Cloudflare account.         |
| `[tunnel.sites]` | map `site → hostname`     | Per-site public hostnames exposed through the named tunnel.   |

Validation rejects empty keys/values (`TunnelEntryEmpty`), a `[tunnel.sites]` hostname that isn't a plausible DNS name (`TunnelHostnameInvalid`), and any key or UUID containing path- or YAML-unsafe characters (`TunnelKeyInvalid`). The account certificate and per-tunnel credentials are **not** stored here - they live in a daemon-owned `0700` directory, never in the config file.

```toml
[tunnel.named]
my-tunnel = "6ff42ae2-765d-4adf-8112-31c55c1551ef"

[tunnel.sites]
app = "app.example.com"
```

### `[groups]`

User-defined site groups for the desktop app's Sites view. Purely an organisational overlay - groups do not affect routing. Both fields are **empty by default**, so the whole `[groups]` table is omitted from the file until you create a group.

| Key       | TOML type        | Meaning                                                        |
| --------- | ----------------- | --------------------------------------------------------------- |
| `order`   | array of strings  | Group display names, in display order.                        |
| `members` | table (`site → group`) | Per-site group membership, keyed by site name.           |

Membership is keyed by **site name**, not document-root, so a group applies to parked and linked sites alike without touching either site's own record. A site absent from `members` is "Unallocated" - the GUI's synthetic bucket for ungrouped sites, which is never itself persisted here.

Validation rules (enforced by `Config::validate`): every name in `order` must be non-empty (`GroupNameEmpty`) and unique, ASCII-case-insensitively (`GroupDuplicate`); the name `Unallocated` is reserved in any casing and rejected (`GroupNameReserved`); and every `members` value must reference a group present in `order`, also folding case (`GroupMemberDangling`). Whether a keyed site still exists is not checked - parked sites are discovered from disk on each scan and have no config record to check against.

```toml
[groups]
order = ["Blog", "Shop"]

[groups.members]
api = "Blog"
```

## Schema versioning and migration

Every config file **must** carry a top-level `version = N` key - it is the single trigger for forward migration. The current schema version is `10`.

When the daemon loads a file, it routes on the version it finds:

```text
found  > CURRENT (10)   →  error (UnsupportedVersion) - a newer Yerd wrote this file
found == CURRENT (10)   →  parse directly
found  < CURRENT (10)   →  walk forward migration steps, then parse
```

A file written by a *newer* Yerd than you are running is refused rather than misread. Older files are migrated forward in place, one version at a time, before the normal wire-deserialisation and validation run:

- **`v1 → v2`** is a bare version bump: v2 only **added** the optional `web_subpath` (`[[linked]]`) and `web_root` (`[[overrides]]`) keys, which default when absent, so a v1 file needs no structural rewrite.
- **`v2 → v3`** is the first *structural* migration: it rewrites the old `[services]` shape (a flat `enabled = ["redis", ...]` array of identifiers) into per-service `[services.<id>]` tables, carrying each previously-enabled id forward as an `enabled = true` instance.
- **`v3 → v4`** is a bare version bump: v4 only **added** the optional `[mail]` section, which defaults when absent, so a v3 file needs no structural rewrite. The bump exists so an *older* binary rejects a file using `[mail]` cleanly as `UnsupportedVersion` rather than failing on the unknown table.
- **`v4 → v5`** is likewise a bare version bump: v5 only **added** the optional `[dumps]` table, which defaults when absent. Same rationale - the bump lets an older binary refuse a `[dumps]`-bearing file cleanly instead of tripping `deny_unknown_fields`.
- **`v5 → v6`** is a bare version bump: v6 only **added** the top-level `update_channel` scalar (defaults to `"stable"` when absent).
- **`v6 → v7`** is a bare version bump: v7 only **added** the `[ports]` `fallback_http` / `fallback_https` keys (defaulting to `8080` / `8443`).
- **`v7 → v8`** is a bare version bump: v8 only **added** the optional `[tunnel]` table, which defaults to empty when absent. Same rationale - the bump lets an older binary refuse a `[tunnel]`-bearing file cleanly rather than tripping `deny_unknown_fields`.
- **`v8 → v9`** is a bare version bump: v9 only **added** the optional `[groups]` table, which defaults to empty when absent. Same rationale - the bump lets an older binary refuse a `[groups]`-bearing file cleanly rather than tripping `deny_unknown_fields`.
- **`v9 → v10`** is a bare version bump: v10 only **added** the optional `[php.extensions]` registry, which defaults to empty when absent. Same rationale - the bump lets an older binary refuse a `[php.extensions]`-bearing file cleanly rather than tripping `deny_unknown_fields`.

The on-disk schema version is deliberately decoupled from the IPC protocol version; the two evolve independently.

::: warning Downgrades are refused, not misread
Because later versions changed shapes the parser checks strictly (keys *inside* `[[linked]]` / `[[overrides]]` in v2, and the whole `[services]` shape in v3), an older daemon reading a newer file would fail. The version routing turns that into a clean `UnsupportedVersion` error instead - downgrading Yerd against a newer config is unsupported, but it fails loudly rather than corrupting state.
:::

::: tip Forward-compatible by design
The parser tolerates older shapes: a v1 file written before `web_subpath`/`web_root` existed migrates to v2 and parses fine (the new fields default). New optional fields are added additively, so upgrades don't break your existing config.
:::

## Atomic saves

Saves are atomic. The daemon serialises the config, writes it to a temporary file in the same directory, then `rename`s it over `yerd.toml`. Because the temp file lives on the same filesystem as the destination, the rename is atomic on Unix - a reader never sees a half-written file, and a crash mid-save leaves the previous config intact. On failure the temp file is cleaned up automatically, so no orphan files are left behind.

On Unix the file is created with mode `0600` (owner read/write only): the daemon is the only intended writer. Intermediate parent directories are created as needed.

::: info Durability trade-off
Yerd does not `fsync` the file or its parent directory after a save. For a developer-only config file the portability cost outweighs the durability gain, so a loss under sudden power loss is accepted by design.
:::

## A complete annotated example

This is a full, valid `yerd.toml` exercising every field:

```toml
# Schema version - mandatory, always written as 10 by this release.
version = 10

# TLD served by the resolver; sites resolve as <name>.test
tld = "test"

# Loopback port for the embedded .test DNS responder (default 1053).
dns_port = 1053

# Proxy listen ports. Defaults are 80 / 443 (may need elevation).
# Swap for the rootless 8080 / 8443 pair to avoid privileged binds.
[ports]
http = 80
https = 443

[php]
# Default PHP version applied to new sites.
default = "8.3"

# Global ini directives written into every installed version's FPM pool.
# Allowlisted directives only; values are validated as a security boundary.
[php.settings]
memory_limit = "512M"
upload_max_filesize = "64M"
post_max_size = "64M"

# Custom extensions, keyed by PHP version and loaded into FPM + CLI.
# Manage with `yerd php ext` (each .so is load-probed before saving).
[[php.extensions."8.5"]]
name = "scrypt"
path = "/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
zend = false

# Parked directories: each immediate subdirectory becomes a site.
# Paths are stored verbatim and are NOT canonicalised.
[parked]
paths = ["/Users/you/Sites"]

# Explicitly linked sites (order preserved). web_subpath is optional (the
# served web root relative to document_root; omitted when the root is served).
[[linked]]
name = "api"
document_root = "/Users/you/projects/api"
web_subpath = "public"
php = "8.3"
secure = true
kind = "linked"

# Per-site overrides for PARKED sites, keyed by exact document-root path.
# Omit php / secure / web_root to inherit / auto-detect. `path` must match the
# scan byte-for-byte.
[[overrides]]
path = "/Users/you/Sites/blog"
php = "8.4"
secure = true
web_root = "public"

# Installed database / cache services, one table per engine.
# Known ids: mysql, mariadb, postgres, redis. Usually managed via `yerd service`.
[services.redis]
version = "8"
port = 6379
enabled = true

# Built-in mail-capture SMTP server. ON by default - this table is written only
# when a value differs from the default (enabled, port 2525); a default config
# omits [mail] entirely. Shown here for completeness.
[mail]
enabled = true
port = 2525

# Laravel ▸ Dumps telemetry. OFF by default - omitted from a default file. When
# present, absent [dumps.features] keys default to ON, so only disabled features
# need listing.
[dumps]
enabled = true
port = 2304
persist = false

[dumps.features]
queries = false
```

## Related pages

- [Sites](../guide/sites) - parking and linking explained
- [PHP Versions](../guide/php-versions) - managing installed versions and per-site PHP
- [HTTPS & Certificates](../guide/https) - what `secure` turns on
- [DNS & .test Domains](../guide/dns) - how `tld` and `dns_port` are used
- [CLI Reference](./cli/) - the commands that edit this file for you
- [yerd-config crate](../developer/crates/yerd-config) - the implementation behind this schema
