# Services

Yerd installs and supervises local database, cache, and search engines as native,
per-user processes - no Docker. Each engine is identified by a short `id`:
`redis`, `mysql`, `mariadb`, `postgres`, or `meilisearch`. The [Services & Databases
guide](../../guide/services) covers the model in depth; this page is the command
reference. For creating and managing the databases *inside* a SQL engine, see
[Databases](./db).

::: info Redis is Valkey
The `redis` slot is served by **Valkey** (the BSD-licensed, wire-compatible fork).
It is displayed as `Redis (Valkey)` and your clients are unaffected.
:::

## Listing

| Command | Description |
| --- | --- |
| `yerd services` | List every known service: installed version, run state (running / stopped / failed), port, and whether it hosts databases. |
| `yerd service available` | List the versions installable from Yerd's hosted distribution for your platform, tagging any already installed. |

```sh
yerd services             # what's installed and running
yerd service available    # what you could install
```

## Installing & versioning

| Command | Description | Example |
| --- | --- | --- |
| `yerd service install <SVC> <VERSION>` | Download and install a service build, then start and enable it. | `yerd service install redis 8` |
| `yerd service change-version <SVC> <VERSION>` | Switch an installed service to a different version (the data directory is kept). | `yerd service change-version postgres 16.2` |
| `yerd service uninstall <SVC> <VERSION> [--purge]` | Remove an installed version. Add `--purge` to also delete the engine's stored data (destructive). | `yerd service uninstall mysql 8.4 --purge` |

```sh
yerd service install redis 8           # install + start + enable
yerd service change-version redis 8.1  # upgrade in place, keep data
yerd service uninstall redis 8         # remove binaries, keep data
yerd service uninstall redis 8 --purge # remove binaries AND data
```

::: warning `--purge` deletes data
Without `--purge`, uninstalling keeps the data directory so a later reinstall
picks up where you left off. With `--purge` the engine's stored data is deleted -
there is no undo.
:::

::: info PostgreSQL has a `full` (PostGIS) variant
`postgres` publishes two builds per major: the lean base (`17`) and a PostGIS
build (`17-full`). Install either by its label, e.g.
`yerd service install postgres 17-full`. The two are separate installs that
**share one data directory** (pinned to the numeric major), so `change-version`
between them preserves your databases; see
[PostgreSQL: base and PostGIS builds](../../guide/services#postgresql-base-and-postgis-full-builds)
for the extension lists, the shared-datadir behaviour, and the GPL posture of
`full`.
:::

## Lifecycle

| Command | Description |
| --- | --- |
| `yerd service start <SVC>` | Start the service now. |
| `yerd service stop <SVC>` | Stop the service for the current session. Installed engines auto-start again on the next daemon start; `uninstall` to keep one off. |
| `yerd service restart <SVC>` | Restart the running service. |

```sh
yerd service start postgres
yerd service stop postgres
yerd service restart postgres
```

## Configuration

| Command | Description | Example |
| --- | --- | --- |
| `yerd service set-port <SVC> <PORT>` | Set the loopback port the service listens on. Applies on the next start/restart. | `yerd service set-port redis 6380` |
| `yerd service set <SVC> <KEY> <VALUE>` | Set a free-form config directive for the engine. Applies on the next start/restart. | `yerd service set mysql max_allowed_packet 256M` |
| `yerd service unset <SVC> <KEY>` | Remove a directive Yerd is overriding, so the engine's own default applies again. | `yerd service unset mysql max_allowed_packet` |
| `yerd service overrides <SVC>` | List the directives currently set for a service (`no overrides` when there are none). | `yerd service overrides mysql` |
| `yerd service logs <SVC> [--lines <N>]` | Print the tail of the service's log. `--lines` defaults to 100. | `yerd service logs mysql --lines 200` |

```sh
yerd service set-port redis 6380
yerd service logs mysql              # last 100 lines
yerd service logs mysql --lines 50
```

Default ports: Redis `6379`, MySQL / MariaDB `3306` (they share the port, so only
one can be enabled on it at a time), PostgreSQL `5432`, Meilisearch `7700`.

### Configuration overrides

`set` / `unset` / `overrides` manage free-form directives for the engine's *own*
config file - the way `yerd php ini` does for a PHP version. Yerd renders them
into a sidecar the engine reads after Yerd's own settings, so an override wins:

```sh
yerd service set mysql max_allowed_packet 256M
yerd service set mysql sql_mode STRICT_TRANS_TABLES,NO_ZERO_DATE
yerd service overrides mysql
#   max_allowed_packet = 256M
#   sql_mode = STRICT_TRANS_TABLES,NO_ZERO_DATE
yerd service unset mysql sql_mode
yerd service restart mysql           # overrides apply on the next start
```

Supported by the config-backed engines only: `mysql`, `mariadb`, `postgres`, and
`redis`. Meilisearch and Reverb are argv/env driven, so they answer
`does not support configuration overrides`.

Names and values are **shape-validated** client-side before connecting (and again
by the daemon), but not semantically: whether the engine accepts a directive is
the engine's business, and a bad one surfaces when the service next starts.
Directives Yerd manages through typed paths are refused with a pointer to the
right command - the port (use `yerd service set-port`), the data directory, the
socket, the pid file, logging (read it with `yerd service logs`), the
MySQL/MariaDB bootstrap `init-file`, the loopback binding, and the engines' own
`include` directives. The check folds case in every dialect, and `-`/`_` for
MySQL/MariaDB, so `Bind_Address` is refused just as `bind-address` is.

::: warning Restart to apply
Like `set-port`, setting an override never restarts anything. Run
`yerd service restart <SVC>` when you're ready for it to take effect. If the
engine then refuses to start, the error carries the tail of its own log plus the
path to the hand-edit file - see the
[Services & Databases guide](../../guide/services#getting-a-directive-wrong).
:::

Hand edits that Yerd must never touch go in the service's `conf.d/50-local.<ext>`
file instead, which is created once and never rewritten. `yerd doctor` scans it
and warns about reserved or malformed lines. See
[Service configuration overrides](../../guide/services#service-configuration-overrides)
for the two-file model, and the [Configuration
Reference](../configuration#services-id) for how overrides are stored.

## See also

- [Services & Databases guide](../../guide/services) - the supervision model and posture
- [Databases](./db) - creating, dropping, backing up databases inside a SQL engine
- [Configuration Reference](../configuration) - the `[services.<id>]` config tables
- [yerd-services](../../developer/crates/yerd-services) - the crate behind these commands
