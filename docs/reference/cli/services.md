# Services

Yerd installs and supervises local database and cache engines as native,
per-user processes - no Docker. Each engine is identified by a short `id`:
`redis`, `mysql`, `mariadb`, or `postgres`. The [Services & Databases
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
build (`17-full`). Install either by its label, e.g. `yerd service install
postgres 17-full`. The two are separate installs that **share one data directory**
(pinned to the numeric major), so `change-version` between them preserves your
databases; see
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
| `yerd service logs <SVC> [--lines <N>]` | Print the tail of the service's log. `--lines` defaults to 100. | `yerd service logs mysql --lines 200` |

```sh
yerd service set-port redis 6380
yerd service logs mysql              # last 100 lines
yerd service logs mysql --lines 50
```

Default ports: Redis `6379`, MySQL / MariaDB `3306` (they share the port, so only
one can be enabled on it at a time), PostgreSQL `5432`.

## See also

- [Services & Databases guide](../../guide/services) - the supervision model and posture
- [Databases](./db) - creating, dropping, backing up databases inside a SQL engine
- [Configuration Reference](../configuration) - the `[services.<id>]` config tables
- [yerd-services](../../developer/crates/yerd-services) - the crate behind these commands
