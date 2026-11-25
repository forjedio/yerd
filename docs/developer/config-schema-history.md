# Config schema history

`yerd.toml`'s on-disk schema is versioned independently of everything else - the IPC wire protocol, the app version, the daemon binary. This page is the version-by-version changelog: what each schema version added, whether the daemon can migrate a file forward automatically, and - the reason this page exists - **exactly what to change by hand if you need to downgrade** a config file so an older Yerd build will accept it again.

For how the versioning *mechanism* works (the `STEPS` array, `deny_unknown_fields`, the purity boundary), see [yerd-config](./crates/yerd-config#schema-versioning-and-migration). For the field-by-field "how do I configure X" reference, see the [Configuration Reference](../reference/configuration).

## Where the file lives

`yerd.toml` sits in the OS-standard config directory for the `io.yerd.Yerd` app, resolved once at startup by [`yerd-platform`](./crates/yerd-platform)'s `PlatformDirs`:

| OS | Default config directory | Full default path |
| --- | --- | --- |
| macOS | `~/Library/Application Support/io.yerd.Yerd` | `~/Library/Application Support/io.yerd.Yerd/yerd.toml` |
| Linux | `$XDG_CONFIG_HOME/yerd` (falls back to `~/.config/yerd` when unset) | `~/.config/yerd/yerd.toml` |
| Windows | Not yet supported (`os::unsupported` stub) | n/a |

::: info Overriding the path
`yerdd serve -c <path>` (`--config <path>`) points the daemon at a different file entirely - useful for testing a downgraded copy without touching your real config. A missing file is not an error: the daemon boots with `Config::default()` (a fresh, empty config) and logs that it's using defaults for a first-run boot. Anything else - invalid TOML, a version the daemon doesn't understand, a value that fails validation - is fatal; the daemon refuses to start rather than silently discarding your settings.
:::

macOS's `config`, `data`, and `state` directories all coincide at the same `io.yerd.Yerd` bundle (no XDG-style state/data/config split); Linux keeps them genuinely separate per the XDG base-directory spec, so `yerd.toml` (config) is not near the CA certificate or PHP installs (data) or the daemon's runtime state.

## How to read this page

Every on-disk file **must** carry a top-level `version = N` key - there is no "unversioned" file, and a missing key is a hard parse error. The daemon migrates a file **forward only**, one version at a time, the moment it loads it; there is no automatic downgrade path. A file whose version is *newer* than what a given Yerd build understands is rejected cleanly as `UnsupportedVersion` (a clear error naming both versions) rather than being partially parsed or silently corrupted - so an old binary reading a new file always fails safely. That rejection is also *why* you'd want this page: to hand-edit a newer file back down so an older build can read it, rather than losing your settings and starting from a blank config.

Each entry below states what changed, whether the daemon's own migration is a bare version-number bump (nothing else in the file needs to change to move forward) or a structural rewrite, and - under **To downgrade** - the exact manual edit that reverses it.

## Version-by-version

### v33 (current)

**Added:** the optional `[response_headers.proxy]` table - custom response headers for whole-host proxies, keyed by proxy name, in the same `name`/`value` array-of-tables shape as `[response_headers.linked]`. It defaults to empty when absent, so a file with no proxy headers omits it entirely. A key naming no configured proxy is tolerated (as in `[domains.proxy]`): it is neither routed nor listed, and the daemon prunes a proxy's entry when the proxy is removed.

```toml
[[response_headers.proxy.reverb]]
name = "Access-Control-Allow-Origin"
value = "*"
```

**Migration from v32:** bare version bump - the table defaults to empty when absent, so a v32 file needs no other change.

**To downgrade to v32:** change `version = 33` to `version = 32` and delete every `[[response_headers.proxy.*]]` table (a v32 daemon rejects the unknown key under `deny_unknown_fields`).

### v32

**Added:** `default_provider` under `[tunnel]` (`cloudflare`, `ngrok` or `zrok`; the provider new shares use, default `cloudflare` and omitted while it is) and the optional `[tunnel.ngrok]` table (`authtoken`, the ngrok agent authtoken, and `domain`, an optional static domain as a bare hostname). Both default when absent, so a file that never configured a provider omits them entirely.

```toml
[tunnel]
default_provider = "ngrok"

[tunnel.ngrok]
authtoken = "2fXk..."
domain = "demo.ngrok-free.app"
```

**Migration from v31:** bare version bump - both default when absent, so a v31 file needs no other change.

**To downgrade to v31:** change `version = 32` to `version = 31`, delete the `default_provider` line from `[tunnel]`, and delete the `[tunnel.ngrok]` table (a v31 daemon rejects both under `deny_unknown_fields`).

### v31

**Added:** the optional `[parked.claims]` map - the suffix-stripped site names held by parked folders, keyed by byte-exact document root, each with the `name` held and a `url_synced` flag. The daemon maintains it after every scan. It defaults to empty when absent, so a file with no domain-named parked folders omits it entirely.

```toml
[parked.claims."/Users/you/Sites/laravel.com"]
name = "laravel"
url_synced = true
```

**Migration from v30:** bare version bump - the map defaults to empty when absent, so a v30 file needs no other change.

**To downgrade to v30:** change `version = 31` to `version = 30` and delete every `[parked.claims."..."]` table (a v30 daemon rejects the unknown key under `deny_unknown_fields`).

A v30 daemon names domain-named folders by their full slug again (`laravel-com.test` rather than `laravel.test`), so anything that followed a site to its shorter name stays behind:

- **Settings keyed by the short name** do not apply under v30: `[favourites]` entries, `[groups.members]` keys, `[tunnel.sites]` and `[tunnel.passwords]` keys, and per-site `[services."<type>:<site>"]` instances. To keep them, rename each key from the short name to the full slug (`laravel` to `laravel-com`; a per-site service id becomes `"reverb:laravel-com"`, and its `site` field too).
- **App-side URLs** keep the short host. Point WordPress at the full slug (`wp option update siteurl https://laravel-com.test` and the same for `home`) and set `app.baseURL` in a CodeIgniter site's `.env` to the full-slug URL.

The `[domains.parked]` deltas are keyed by document root, so they carry over; only an apex entry naming the short host needs changing back.

### v30

**Added:** the optional `[favourites]` table - the sites starred in the desktop app, as a list of lowercased site names. Entries are not validated against live sites, like `[groups]` membership. It defaults to empty when absent, so a file with no favourites omits it entirely.

```toml
[favourites]
sites = ["blog"]
```

**Migration from v29:** bare version bump - the table defaults to empty when absent, so a v29 file needs no other change.

**To downgrade to v29:** change `version = 30` to `version = 29` and delete the `[favourites]` table (a v29 daemon rejects the unknown table under `deny_unknown_fields`). The starred sites are lost.

### v29

**Added:** the optional `[php.xdebug]` table - per-version settings for the [bundled Xdebug](../guide/xdebug) toggle. `[php.xdebug."<version>"]` holds `enabled` (bool), `modes` (a non-empty list of `debug`, `develop`, `coverage`, `profile`, `trace`, `gcstats`) and `start` (`"always"` or `"trigger"`). An entry is written only while it differs from the default (off, `["debug"]`, `"always"`), so an untouched file omits the table entirely.

```toml
[php.xdebug."8.4"]
enabled = true
modes = ["debug"]
start = "always"
```

The table loads **leniently**: an entry with an unparseable or empty mode list, a bad or missing `start`, or a legacy (< 8.2) version key is dropped during parsing rather than failing the load. A malformed version key or an unknown key inside an entry is still a hard error.

**Migration from v28:** bare version bump - the table defaults to empty when absent, so a v28 file needs no other change.

**To downgrade to v28:** change `version = 29` to `version = 28` and delete the `[php.xdebug]` table itself along with any `[php.xdebug.*]` tables (a v28 daemon rejects the unknown table under `deny_unknown_fields`, even an empty one). Every pool then starts without Xdebug. The downloaded `{data}/php-ext/php-<version>/xdebug.so` and `debug.ini` files are inert leftovers you can delete.

### v28

**Added:** the optional `[network]` table - the outbound HTTP proxy for Yerd's own downloads and the tools it runs: `mode` (`"system"`, `"manual"` or `"off"`), `http_proxy`, `https_proxy` and `no_proxy`. It is omitted while it holds the defaults (system detection, nothing manual).

```toml
[network]
mode = "manual"
http_proxy = "http://proxy.corp:3128"
no_proxy = [".corp.com"]
```

**Migration from v27:** bare version bump - the table defaults to system detection when absent, so a v27 file needs no other change.

**To downgrade to v27:** change `version = 28` to `version = 27` and delete the `[network]` table if present (a v27 daemon rejects the unknown table under `deny_unknown_fields`). An older Yerd ignores proxies set only in Yerd; it still picks up nothing from the system on its own, so downloads go direct.

### v27

**Added:** the optional `[node]` table (`default`, the global Node selector) and `[node.sites]` (per-project Node preferences keyed by canonical absolute project root). Both default to empty when absent; a legacy single-version Node install stays discoverable and acts as the default until one is saved.

```toml
[node]
default = "24.2.0"

[node.sites]
"/srv/example" = "22.9.0"
```

**Migration from v26:** bare version bump - the table defaults to empty when absent, so a v26 file needs no other change.

**To downgrade to v26:** change `version = 27` to `version = 26` and delete the `[node]` table if present (a v26 daemon rejects the unknown table under `deny_unknown_fields`). Saved Node defaults and per-project preferences are lost.

### v26

**Added:** the optional `ffi.enable` key in `[php.settings]` and `[php.version_settings."<version>"]` - the "Allow FFI in web requests" switch. It accepts an on/off value (stored as `On`/`Off`) and is never seeded, so a file that has not turned it on omits it. When on, PHP 8.2+ pools get `php_admin_value[ffi.enable] = true`; when absent or `Off`, PHP's own `preload` default applies. It is refused for legacy (7.4 to 8.1) per-version overrides.

```toml
[php.settings]
"ffi.enable" = "On"
```

**Migration from v25:** bare version bump - the key is absent unless set, so a v25 file needs no other change.

**To downgrade to v25:** change `version = 26` to `version = 25` and delete any `"ffi.enable"` keys (a v25 daemon rejects the unknown name in `[php.settings]`). Web requests fall back to `ffi.enable=preload`.

### v25

**Added:** the optional `[response_headers]` table - per-site **custom response headers**, keyed by site class exactly like `[route_rules]`: `[response_headers.linked.<name>]` by site name, `[response_headers.parked."<docroot>"]` by document-root string. Each entry pairs a header `name` with a `value`. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[[response_headers.linked.api]]
name = "Access-Control-Allow-Origin"
value = "https://app.test"
```

The proxy sets every listed header on every response it produces for the site, replacing any same-named header the app or Yerd already set (issue #234). Each entry is validated at load: the name must be a valid HTTP header name and not one Yerd manages (`Connection`, `Keep-Alive`, `Transfer-Encoding`, `Upgrade`, `TE`, `Trailer`, `Content-Length`, `Host`, `Set-Cookie`, `Content-Encoding`, `Content-Range`, `Location`, or any `Proxy-*`), and the value must be non-empty with no control characters. A hand-edited entry that fails these checks is a hard parse error, exactly like a bad `[route_rules]` target. Two entries for one site with the same name (ignoring case) fail validation.

**Migration from v24:** bare version bump - the table defaults to empty when absent, so a v24 file needs no other change.

**To downgrade to v24:** change `version = 25` to `version = 24` and delete any `[response_headers.*]` tables (a v24 daemon rejects the unknown table under `deny_unknown_fields`). Sites stop sending the custom headers.

### v24

**Added:** the optional `[tunnel.passwords]` table - per-site share passwords, keyed by site name. When a site has a password, every tunnelled request to it (quick or named tunnel) must carry HTTP basic auth with that password; the username is ignored. One password protects every tunnel kind for that site. Each value is 1 to 128 bytes with no control characters, and keys follow the same safe-key rule as `[tunnel.sites]`. It defaults to empty when absent, so a file with no passwords omits it entirely. The file is saved `0600`, and passwords are never returned over IPC.

```toml
[tunnel.passwords]
blog = "let-me-in"
```

**Migration from v23:** bare version bump - the table defaults to empty when absent, so a v23 file (including existing `[tunnel.named]` and `[tunnel.sites]` tables) needs no other change.

**To downgrade to v23:** change `version = 24` to `version = 23` and delete the `[tunnel.passwords]` table if present (a v23 daemon rejects the unknown key under `deny_unknown_fields`). Shared sites are then served without a password.

### v23

**Added:** the optional `[domains.proxy]` table - routable-domain deltas for **whole-host proxies**, keyed by proxy name. It sits alongside the existing `[domains.linked]` (by site name) and `[domains.parked]` (by document-root string) maps and carries the same three keys: `added`, `suppressed`, and `primary`. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[domains.proxy.account-dev]
added = ["custom-domain", "*.account-dev"]
primary = "custom-domain"
```

A proxy name may itself be dotted (`api.account`), in which case TOML quotes the key: `[domains.proxy."api.account"]`. Domains are stored as **sub-parts** below the TLD, exactly as for sites, so `custom-domain` here means `custom-domain.test`. An all-empty delta is pruned by the writer, and a key naming no current proxy is inert rather than an error - the same tolerance `[domains.linked]` already has.

**Migration from v22:** bare version bump - the table defaults to empty when absent, so a v22 file needs no other change.

**To downgrade to v22:** change `version = 23` to `version = 22` and delete any `[domains.proxy.*]` tables (a v22 daemon rejects the unknown table under `deny_unknown_fields`, it doesn't just ignore it). Each proxy reverts to answering on its apex only, `<name>.test`.

### v22

**Added:** the optional `[services.<id>.overrides]` sub-table - free-form configuration overrides for a service instance, keyed by directive name. Each entry is written into that engine's generated `conf.d/10-yerd.<ext>` sidecar on every start, so the settings survive the restart that regenerates the main config (issue #195). Only the config-backed engines accept them (`mysql`, `mariadb`, `postgres`, `redis`); the table is dropped at load for any other service. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[services.mysql.overrides]
max_allowed_packet = "256M"
sql_mode = "STRICT_TRANS_TABLES,NO_ZERO_DATE"
```

Entries are shape-validated, not semantically validated: a name or value that could break out of the generated option file is refused when set, and an entry naming a directive Yerd owns (`port`, `datadir`, `bind-address`, …) is refused with a hint pointing at the typed command that manages it. At **load** time the same checks run leniently - a bad entry is dropped rather than failing the whole file - so hand-editing this table can never make the daemon refuse to start.

Hand edits belong in the sibling `conf.d/50-local.<ext>` file instead, which Yerd creates once and never rewrites; it is read after `10-yerd.<ext>`, so it wins.

**Migration from v21:** bare version bump - the table defaults to empty when absent, so a v21 file needs no other change.

**To downgrade to v21:** change `version = 22` to `version = 21` and delete any `[services.<id>.overrides]` tables. Those overrides stop being written to `conf.d/10-yerd.<ext>`, so the affected engines fall back to Yerd's generated defaults on the next restart. Anything you put in `conf.d/50-local.<ext>` is unaffected by the downgrade, but an older build emits no include line for it, so it stops being read until you upgrade again.

### v21

**Added:** the optional `[route_rules]` table - per-site path-prefix **routing** rules, keyed by site class exactly like `[proxy_rules]` and `[domains]`: `[route_rules.linked.<name>]` by site name, `[route_rules.parked."<docroot>"]` by document-root string. Each rule pairs a URI path `prefix` with a `target` path relative to the site's served root. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[[route_rules.linked.portal]]
prefix = "/api"
target = "api/index.php"

[[route_rules.linked.dashboard]]
prefix = "/"
target = "index.html"
```

A rule applies only when the request matched no real file, i.e. nginx's `try_files $uri $uri/ <target>`. A `.php` target is a nested front controller (issue #196: a Yii or CodeIgniter app mounted inside a legacy portal); any other target is served as a static document, which is how SPA history-API routing works. The `target` is validated as a safe relative path at load, so a hand-edited absolute path or one containing `..` is a hard parse error rather than a silent security hole.

Note this is **not** `[proxy_rules]`: a proxy rule forwards to an HTTP upstream, a routing rule resolves to a file inside the site's own tree.

**Migration from v20:** bare version bump - the table defaults to empty when absent, so a v20 file needs no other change.

**To downgrade to v20:** change `version = 21` to `version = 20` and delete any `[route_rules.*]` tables (a v20 daemon rejects the unknown tables under `deny_unknown_fields`, it doesn't just ignore them). Sites revert to funnelling unmatched requests to the served root's `index.php`.

### v20

**Added:** the optional `[php.pool]` table - per-version FPM pool settings. `[php.pool."<version>"]` holds the pool settings for one installed version; the only key is `max_children`, the ceiling on concurrent PHP workers, accepted between `1` and `1024` and defaulting to `16`. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[php.pool."8.4"]
max_children = "32"
```

These are FPM pool-block settings rather than ini directives, so they reach the generated pool config only, never a CLI `php.ini`. The `pm.` prefix is reserved out of `[php.directives]` for the same reason: rendered there it would become `php_value[pm.max_children]`, which FPM refuses on every worker spawn.

The table loads **leniently**, like `[php.directives]`: an out-of-range value or an unknown setting name is dropped during parsing rather than failing the load. A malformed version key is still a hard error, and strict validation lives at set time (CLI/GUI/IPC).

**Migration from v19:** bare version bump - the table defaults to empty when absent, so a v19 file needs no other change.

**To downgrade to v19:** change `version = 20` to `version = 19` and delete any `[php.pool.*]` tables (an older daemon rejects the unknown tables under `deny_unknown_fields`, it doesn't just ignore them). Every version falls back to the built-in ceiling of 16.

### v19

**Added:** the top-level `lan_enabled` and `lan_setup_port` scalars, gating LAN exposure (serving your `.test` sites to other devices on the network) and setting the port the one-time remote-device setup page listens on. Both default when absent - `lan_enabled = false`, so LAN exposure stays opt-in, and `lan_setup_port = 7073`.

```toml
lan_enabled = false
lan_setup_port = 7073
```

**Migration from v18:** bare version bump - both scalars default when absent, so a v18 file needs no other change.

**To downgrade to v18:** change `version = 19` to `version = 18` and delete the `lan_enabled` and `lan_setup_port` lines (a v18 daemon rejects the unknown keys under `deny_unknown_fields`, it doesn't just ignore them). LAN exposure is off on a v18 daemon regardless.

### v18

**Added:** the optional `[php.directives]` table - free-form (shape-validated) per-version ini directives such as `xdebug.mode`. `[php.directives."<version>"]` holds the directives for one installed version. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[php.directives."8.3"]
"xdebug.mode" = "debug"
```

The table loads **leniently**: an invalid or reserved entry (e.g. from a hand-edit) is dropped during parsing rather than failing the load, so a bad entry can never stop the daemon. A malformed version key is still a hard error, and the strict validation lives at set time (CLI/GUI/IPC).

**Migration from v17:** bare version bump - the table defaults to empty when absent, so a v17 file needs no other change.

**To downgrade to v17:** change `version = 18` to `version = 17` and delete any `[php.directives.*]` tables (a v17 daemon rejects the unknown tables under `deny_unknown_fields`, it doesn't just ignore them). Every version falls back to the `[php.settings]` and `[php.version_settings]` values.

### v17

**Added:** the top-level `mcp_enabled` scalar (bool) - whether `yerd mcp` serves Yerd's tools to local AI agents. Defaults to `false`, so exposing Yerd to agents is an explicit opt-in, and is always emitted.

```toml
mcp_enabled = false
```

The daemon runs no MCP server of its own: it stores this flag and reports it in the status report, and each `yerd mcp` session reads it to decide whether to serve. See [AI Agents](../guide/ai-agents).

**Migration from v16:** bare version bump - the key defaults to `false` when absent, so a v16 file needs no other change.

**To downgrade to v16:** change `version = 17` to `version = 16` and delete the `mcp_enabled` line (a v16 daemon rejects the unknown key under `deny_unknown_fields` rather than ignoring it). Agents lose access to Yerd's tools; nothing else is affected.

### v16

**Added:** the optional `[php.version_settings]` table - per-version overrides of the global PHP settings. `[php.version_settings."<version>"]` holds sparse overrides of the allowlisted `[php.settings]` directives for one installed version. It defaults to empty when absent, so an uncustomised file omits it entirely.

```toml
[php.version_settings."8.3"]
memory_limit = "1G"
```

The table loads **leniently**: an invalid entry (e.g. from a hand-edit) is dropped during parsing rather than failing the load, so a bad entry can never stop the daemon. A malformed version key is still a hard error, and the strict validation lives at set time (CLI/GUI/IPC).

**Migration from v15:** bare version bump - the table defaults to empty when absent, so a v15 file needs no other change to become a valid v16 file.

**To downgrade to v15:** change `version = 16` to `version = 15`, then delete any `[php.version_settings.*]` tables (a v15 daemon rejects the unknown tables under `deny_unknown_fields`, it doesn't just ignore them). Every version falls back to the global `[php.settings]` values.

### v15

**Added:** the multi-instance services rework - the optional per-instance `site` field inside `[services.<id>]` tables and the `"{type}:{site}"` wire ids for per-site app servers, plus a behaviour change: the `enabled` flag now actually gates boot autostart (before v15 every installed engine auto-started regardless).

**Migration from v14:** structural but small - the migration marks every existing single-instance engine (colon-free `[services.<id>]` key) `enabled = true`, so engines installed before the upgrade keep starting with Yerd now that the flag is enforced.

**To downgrade to v14:** change `version = 15` to `version = 14`, then delete any per-site `[services."<type>:<site>"]` tables and any `site = "..."` lines (a v14 daemon rejects the unknown field under `deny_unknown_fields`). Single-instance `enabled` flags survive; a v14 daemon auto-starts every installed engine regardless of the flag.

### v14

**Added:** the optional `[[proxies]]` array (whole-host reverse proxies) and the `[proxy_rules]` table (per-site path-prefix rules). Both default to empty when absent, so an uncustomised file omits them entirely.

```toml
[[proxies]]
name = "reverb"
target = "http://127.0.0.1:8080"
secure = true

[[proxy_rules.linked.myapp]]
prefix = "/app"
target = "http://127.0.0.1:8080"
```

`[proxy_rules]` is split by site class exactly like `[domains]`: `[proxy_rules.linked.<name>]` keys by site name, `[proxy_rules.parked."<docroot>"]` by byte-exact document-root. Removing a site's last rule drops its key, so the file round-trips byte-identically.

**Migration from v13:** bare version bump - both sections default to empty when absent, so a v13 file needs no other change to become a valid v14 file.

**To downgrade to v13:** change `version = 14` to `version = 13`, then delete any `[[proxies]]` entries and the whole `[proxy_rules]` table (a v13 daemon rejects the unknown tables under `deny_unknown_fields`, it doesn't just ignore them). Those proxies and rules stop being served; the sites they were attached to are otherwise unaffected.

### v13

**Added:** the optional per-site `front_controller` key (bool) inside `[[linked]]` and `[[overrides]]` - the toggle between single-front-controller mode (every request funnels through the site-root `index.php`) and direct script execution (a named `.php` under the served root runs directly). Absent = auto: a framework served from a subdirectory (non-empty `web_subpath`) defaults to front-controller mode; WordPress (any layout) and plain root-served sites default to direct execution.

```toml
[[overrides]]
path = "/home/me/projects/blog"
front_controller = false
```

**Migration from v12:** bare version bump - the field defaults to auto when absent, so a v12 file needs no other change to become a valid v13 file.

**Security note (behaviour change on upgrade):** because the default is auto, a plain root-served site (a parked directory that is a whole project, not just its `public/` dir) flips from single-front-controller mode to **direct execution** on upgrade. Any real `.php` under its served root - a stray `phpinfo.php`, `adminer.php`, an old admin tool, a vendored dev script - becomes directly URL-executable where it was previously funnelled to `index.php`. If the site is exposed beyond loopback (a tunnel), those files become remotely reachable. To keep the old behaviour, set `front_controller = true` on the site (`yerd front-controller <name> on`), or point its `web_root` at a clean public directory. Unknown paths still fall back to `index.php`, so custom-router apps are unaffected.

**To downgrade to v12:** change `version = 13` to `version = 12`, then delete any `front_controller` lines (a v12 daemon rejects the unknown key under `deny_unknown_fields`, it doesn't just ignore it).

### v12

**Added:** the top-level `symlink_protection` scalar (bool) - the global toggle for the proxy's symlink-escape guard. `true` (the default) blocks assets/scripts reached via a symlink resolving outside a site's document root; `false` serves them.

```toml
version = 12
symlink_protection = false
```

**Migration from v11:** bare version bump - the field defaults to `true` when absent, so a v11 file needs no other change to become a valid v12 file.

**To downgrade to v11:** change `version = 12` to `version = 11`, then delete the `symlink_protection` line (a v11 daemon rejects the unknown key under `deny_unknown_fields`, it doesn't just ignore it).

### v11

**Added:** the optional top-level `[domains]` table - per-site domain sets (a primary domain plus aliases, subdomains, and single-label wildcards). Split into `[domains.linked.<name>]` (keyed by site name) and `[domains.parked."<docroot>"]` (keyed by byte-exact document-root), each a delta of `added` / `suppressed` / `primary` stored as TLD-relative sub-parts. See the [Configuration Reference](../reference/configuration#domains) for the field-by-field shape.

```toml
[domains.linked.blog]
added = ["corp", "*.blog"]
suppressed = ["blog"]
primary = "corp"
```

**Migration from v10:** bare version bump - `[domains]` defaults to empty when absent, so a v10 file needs no other change to become a valid v11 file.

**To downgrade to v10:** change `version = 11` to `version = 10` and delete the entire `[domains]` table (including every `[domains.linked.*]` / `[domains.parked.*]` sub-table). Each affected site reverts to answering only its default apex `<name>.<tld>`; a v10 daemon rejects the `[domains]` key under `deny_unknown_fields` rather than ignoring it.

### v10

**Added (two independent, optional additions):**

1. The `wp_auto_login` (bool) and `wp_auto_login_user` (string) keys, inside both `[[linked]]` entries and `[[overrides]]` entries - one-click, pre-authenticated `WordPress` admin login, opt-in per site.
2. The `[php.extensions]` registry - custom `.so` extensions to load into both FPM and the CLI, keyed by PHP version and written as an array-of-tables per version.

```toml
[[linked]]
name = "blog"
document_root = "/Users/you/code/blog"
php = "8.3"
secure = true
kind = "linked"
wp_auto_login = true
wp_auto_login_user = "editor"

[[php.extensions."8.5"]]
name = "scrypt"
path = "/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
zend = false
```

**Migration from v9:** bare version bump - both additions default to absent/empty when missing, so a v9 file needs no other change to become a valid v10 file.

**To downgrade to v9:** change `version = 10` to `version = 9`, then delete every `wp_auto_login`/`wp_auto_login_user` line from `[[linked]]`/`[[overrides]]` entries and remove any `[[php.extensions.*]]` tables (a v9 daemon rejects those keys under `deny_unknown_fields`, it doesn't just ignore them).

### v9

**Added:** the optional `[groups]` table - the desktop app's site-grouping overlay (cosmetic only; never affects routing).

```toml
[groups]
order = ["Client work", "Personal"]

[groups.members]
blog = "Personal"
shop = "Client work"
```

**Migration from v8:** bare version bump - `[groups]` defaults to empty when absent.

**To downgrade to v8:** change `version = 9` to `version = 8` and delete the entire `[groups]` table (including `[groups.members]`). You'll lose the group assignments; sites themselves are unaffected.

### v8

**Added:** the optional `[tunnel]` table - Cloudflare Tunnel sharing state (named tunnels and per-site hostnames).

```toml
[tunnel]
named = { my-tunnel = "1a2b3c4d-uuid" }

[tunnel.sites]
blog = "blog.example.com"
```

**Migration from v7:** bare version bump - `[tunnel]` defaults to empty when absent.

**To downgrade to v7:** change `version = 8` to `version = 7` and delete the `[tunnel]` table (and any `[tunnel.sites]`/`[tunnel.named]` sub-tables). Any active shared-tunnel sites will need reconfiguring after you're back on the older build.

### v7

**Added:** `fallback_http` and `fallback_https` keys inside `[ports]` (the rootless-fallback port pair, `8080`/`8443` by default, used when `80`/`443` need elevation).

```toml
[ports]
http = 80
https = 443
fallback_http = 8080
fallback_https = 8443
```

**Migration from v6:** bare version bump - both keys default to `8080`/`8443` when absent.

**To downgrade to v6:** change `version = 7` to `version = 6` and delete the `fallback_http`/`fallback_https` lines from `[ports]` (keep `http`/`https` - those predate v7).

### v6

**Added:** the top-level `update_channel` scalar (self-update channel selector, e.g. `"stable"`).

```toml
version = 6
update_channel = "stable"
```

**Migration from v5:** bare version bump - `update_channel` defaults to `"stable"` when absent.

**To downgrade to v5:** change `version = 6` to `version = 5` and delete the top-level `update_channel = "..."` line.

### v5

**Added:** the optional `[dumps]` table - Laravel ▸ Dumps telemetry capture settings.

```toml
[dumps]
enabled = true
port = 2304
persist = false

[dumps.features]
queries = true
jobs = false
```

**Migration from v4:** bare version bump - `[dumps]` defaults to disabled/empty when absent.

**To downgrade to v4:** change `version = 5` to `version = 4` and delete the `[dumps]` table (including `[dumps.features]`).

### v4

**Added:** the optional `[mail]` table - the built-in mail-capture SMTP server's `enabled`/`port` settings.

```toml
[mail]
enabled = true
port = 2525
```

**Migration from v3:** bare version bump - `[mail]` defaults to enabled on the default port when absent.

**To downgrade to v3:** change `version = 4` to `version = 3` and delete the `[mail]` table.

### v3

**Added:** nothing new - this is the one **structural** migration in the whole history. Every other version bump only adds optional keys; this one rewrites an existing one.

**Before (v0-v2):**

```toml
[services]
enabled = ["mysql", "redis"]
```

**After (v3+):**

```toml
[services.mysql]
enabled = true

[services.redis]
enabled = true
```

Each previously-listed service id becomes its own table with `enabled = true`; a service's `version`/`port` overrides (added independently, not tied to this migration) live inside that same per-service table.

**Migration from v2:** the daemon rewrites the flat `enabled = [...]` array into per-service tables automatically, then bumps the version - this is not a bare bump, but it *is* fully automatic and lossless (nothing to hand-edit going forward).

**To downgrade to v2:** change `version = 3` to `version = 2`, then manually reverse the rewrite: collect every `[services.<id>]` table whose `enabled` is `true` back into a flat array, and delete the per-service tables entirely.

```toml
version = 2

[services]
enabled = ["mysql", "redis"]
```

::: warning Downgrading past v3 loses per-service settings
A per-service `version`/`port` override (e.g. pinning Redis to a specific version, or a custom port) has nowhere to live in the v0-v2 shape - only the flat enabled-ids list survives. Note those values elsewhere before downgrading past v3 if you need them back later.
:::

### v2

**Added:** the optional `web_subpath` key inside `[[linked]]` entries, and `web_root` inside `[[overrides]]` entries - the served web-root override (e.g. `public/` for a Laravel project), independent of automatic detection.

```toml
[[linked]]
name = "blog"
document_root = "/Users/you/code/blog"
web_subpath = "public"
php = "8.3"
secure = true
kind = "linked"
```

**Migration from v1:** bare version bump - `web_subpath`/`web_root` default to "auto-detect" when absent.

**To downgrade to v1:** change `version = 2` to `version = 1` and delete any `web_subpath` (from `[[linked]]`) or `web_root` (from `[[overrides]]`) lines. Those sites fall back to Yerd's automatic web-root detection, which is usually - but not guaranteed to be - the same directory.

### v1

The first schema version any shipped build of Yerd actually wrote to disk. No older shipped file exists to migrate from in practice, but v0 is kept reachable in the migration chain for a hand-crafted `version = 0` file.

**To downgrade to v0:** not meaningful - no Yerd build has ever read a v0 file from disk. If you're here, you almost certainly want v1, which every build since the schema was introduced understands.

## Downgrading in practice

1. **Stop the daemon first.** Editing `yerd.toml` while `yerdd` is running risks it being overwritten by the next mutation (any `yerd park`/`yerd secure`/… command, or a GUI action, rewrites the whole file).
2. **Back up the file** before editing - `cp yerd.toml yerd.toml.bak` - so you can restore the newer version if the older build turns out not to be what you needed.
3. **Walk the versions one at a time**, newest to oldest, applying each "To downgrade" step above in order - don't skip straight from v11 to v5, since some steps (structural v3, in particular) need the intermediate shape.
4. **Reinstall/switch to the older Yerd build**, then start the daemon and confirm it comes up clean (check its log output for a config error) before relying on it.

If you'd rather not hand-edit at all: delete `yerd.toml` outright and let the older daemon boot with a fresh default config, then re-park/re-link your sites. That's often faster than a multi-version downgrade if you don't have many customised settings to preserve.

## See also

- [yerd-config crate reference](./crates/yerd-config) - the migration mechanism itself (`STEPS`, wire mirrors, `deny_unknown_fields`).
- [Configuration Reference](../reference/configuration) - the current schema's field-by-field guide.
- [yerd-platform crate reference](./crates/yerd-platform) - `PlatformDirs` and the config/data/state/cache/runtime split.
- [yerdd (daemon)](./binaries/yerdd) - where the config is loaded at startup (`startup::bring_up`).
