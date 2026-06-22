# Services & Databases

Yerd installs and supervises local **database and cache** engines as native,
per-user processes - the way [DBngin](https://dbngin.com) does, but folded into
the same [`yerdd` daemon](./daemon) that already runs your sites, PHP, HTTPS, and
DNS. No Docker, no containers, no VM. A single `yerd status` shows the whole
stack.

The four engines:

| Service | `id` | Kind | Default port |
|---|---|---|---|
| Redis (Valkey) | `redis` | Cache / key-value | 6379 |
| MySQL | `mysql` | SQL database | 3306 |
| MariaDB | `mariadb` | SQL database | 3306 |
| PostgreSQL | `postgres` | SQL database | 5432 |

::: info Redis is served by Valkey
The `redis` slot is filled by **Valkey**, the BSD-licensed fork, because recent
Redis releases are no longer cleanly redistributable. It is wire-compatible, so
your Redis clients work unchanged. Yerd shows it as `Redis (Valkey)`.
:::

::: tip Engine availability
All four engines are implemented end-to-end. Whether a specific engine/version
installs depends on whether a prebuilt build is published for your platform in
Yerd's hosted distribution - run `yerd service available` to see what you can
install right now. MySQL/MariaDB share port 3306, so only one can be enabled on it
at a time.
:::

## How it works

Service support follows the same model as [PHP versions](./php-versions):

- **Native processes, not Docker.** Prebuilt binaries are downloaded on demand
  from Yerd's own hosted distribution, then run as your user on loopback.
- **Supervised.** `yerdd` runs one process per enabled service, restarts it on
  crash with backoff, and reports health - the same supervision substrate the PHP
  pools use ([`yerd-supervise`](../developer/crates/yerd-supervise)).
- **Rootless.** Everything runs as your user with no elevation. See the
  [privilege model](./elevation).
- **Local-dev posture.** Engines bind to loopback only and accept passwordless
  connections from your user. This is convenient for local development and is not
  meant to be exposed to a network.

## In the desktop app

The [desktop app](./desktop-app) surfaces every engine on its **Services** page,
under the **Developer** group in the sidebar. Install a version, then Start /
Stop / Restart it inline - no terminal needed. The daemon auto-starts every
installed engine on boot, so what you install stays running across reboots.

<ThemedImage light="/images/services-light.png" dark="/images/services-dark.png" alt="The Services page in the Yerd desktop app" />

Each installed engine's `⋯` menu offers:

- **Configuration** - copies a ready-made Laravel `.env` for that engine, with a
  database picker that pre-fills `DB_DATABASE` for the SQL engines.
- **Edit port** - change the loopback port (applies on next start).
- **View logs** - tail the service log.
- **Manage databases** - create, drop, back up, and restore databases (SQL
  engines only).
- **Change version** - upgrade in place, keeping your data.
- **Uninstall** - remove the engine.

## From the command line

### Managing services

```sh
yerd service available          # versions installable for your platform
yerd service install redis 8    # download, install, and start
yerd services                   # list everything: version, state, port

yerd service start redis        # start it now
yerd service stop redis         # stop for this session (returns on next daemon start)
yerd service restart redis

yerd service set-port redis 6380   # change the loopback port (next start)
yerd service logs redis --lines 50 # tail the service log

yerd service change-version redis 8.1   # upgrade in place, keep data
yerd service uninstall redis 8          # remove binaries, keep data
yerd service uninstall redis 8 --purge  # remove binaries AND data
```

See the [Services CLI reference](../reference/cli/services) for every flag.

### Managing databases

For the SQL engines (`mysql`, `mariadb`, `postgres`), Yerd can create, drop, list,
back up, and restore databases without you reaching for a separate client. The
engine must be running.

```sh
yerd db create mysql my_app
yerd db list mysql
yerd db backup mysql my_app ./my_app.sql      # plain-SQL dump
yerd db restore mysql my_app ./my_app.sql     # replay into an existing db
yerd db drop mysql my_app
```

Database names are validated to a strict allowlist (letters, digits, and
underscores; must start with a letter or `_`; at most 63 characters) so the
generated SQL is injection-proof. Engine-internal databases are protected and
can't be dropped. `backup` writes to a temp file and atomically renames it, so a
failed dump never clobbers an existing one. See the [Databases CLI
reference](../reference/cli/db) for details.

## Configuration

Installed services are recorded in your [config file](../reference/configuration)
under per-service `[services.<id>]` tables, each carrying the pinned `version`,
the `port`, and an `enabled` flag (a record of the last start/stop intent):

```toml
[services.redis]
version = "8"
port = 6379
enabled = true
```

You normally don't hand-edit this - drive it through the CLI (or the
[desktop app](./desktop-app)), which keeps the config and the running processes in
sync.

### Auto-start on boot

The daemon auto-starts **every installed engine** when it starts (in the
background, so a slow database cold-boot never delays the proxy or DNS). The
`enabled` flag does **not** gate this - a service you `stop` returns on the next
daemon start. To keep an engine off for good, `uninstall` it.

::: tip MySQL and MariaDB share port 3306
Only one can listen on `3306` at a time. If both are installed, whichever binds
first wins and the other logs a non-fatal "port in use" and stays down. Run a
single SQL engine, or give one a different `port`.
:::

## Windows and Redis licensing

[Windows service support is still on the roadmap](../developer/cross-platform)
alongside the rest of the Windows platform work. On macOS and Linux all four
engines run today (subject to a published build for your architecture).

## See also

- [Services CLI reference](../reference/cli/services) and [Databases CLI reference](../reference/cli/db)
- [PHP Versions](./php-versions) - the supervision model services share
- [Configuration Reference](../reference/configuration) - the `[services.<id>]` tables
- [yerd-services](../developer/crates/yerd-services) and [yerd-supervise](../developer/crates/yerd-supervise) - the crates behind this
