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

### v11 (current)

**Added:** the top-level `symlink_protection` scalar (bool) - the global toggle for the proxy's symlink-escape guard. `true` (the default) blocks assets/scripts reached via a symlink resolving outside a site's document root; `false` serves them.

```toml
version = 11
symlink_protection = false
```

**Migration from v10:** bare version bump - the field defaults to `true` when absent, so a v10 file needs no other change to become a valid v11 file.

**To downgrade to v10:** change `version = 11` to `version = 10`, then delete the `symlink_protection` line (a v10 daemon rejects the unknown key under `deny_unknown_fields`, it doesn't just ignore it).

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
3. **Walk the versions one at a time**, newest to oldest, applying each "To downgrade" step above in order - don't skip straight from v10 to v5, since some steps (structural v3, in particular) need the intermediate shape.
4. **Reinstall/switch to the older Yerd build**, then start the daemon and confirm it comes up clean (check its log output for a config error) before relying on it.

If you'd rather not hand-edit at all: delete `yerd.toml` outright and let the older daemon boot with a fresh default config, then re-park/re-link your sites. That's often faster than a multi-version downgrade if you don't have many customised settings to preserve.

## See also

- [yerd-config crate reference](./crates/yerd-config) - the migration mechanism itself (`STEPS`, wire mirrors, `deny_unknown_fields`).
- [Configuration Reference](../reference/configuration) - the current schema's field-by-field guide.
- [yerd-platform crate reference](./crates/yerd-platform) - `PlatformDirs` and the config/data/state/cache/runtime split.
- [yerdd (daemon)](./binaries/yerdd) - where the config is loaded at startup (`startup::bring_up`).
