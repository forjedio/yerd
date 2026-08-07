# Services & Databases

Yerd installs and supervises local **database, cache, and search** engines as native,
per-user processes - the way [DBngin](https://dbngin.com) does, but folded into
the same [`yerdd` daemon](./daemon) that already runs your sites, PHP, HTTPS, and
DNS. No Docker, no containers, no VM. A single `yerd status` shows the whole
stack.

The five engines:

| Service | `id` | Kind | Default port |
|---|---|---|---|
| Redis (Valkey) | `redis` | Cache / key-value | 6379 |
| MySQL | `mysql` | SQL database | 3306 |
| MariaDB | `mariadb` | SQL database | 3306 |
| PostgreSQL | `postgres` | SQL database | 5432 |
| Meilisearch | `meilisearch` | Search index | 7700 |

::: info Redis is served by Valkey
The `redis` slot is filled by **Valkey**, the BSD-licensed fork, because recent
Redis releases are no longer cleanly redistributable. It is wire-compatible, so
your Redis clients work unchanged. Yerd shows it as `Redis (Valkey)`.
:::

::: tip Engine availability
All five engines are implemented end-to-end. Whether a specific engine/version
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
- **Override settings** - add your own engine config directives (config-backed
  engines only; see [below](#service-configuration-overrides)).
- **Manage databases** - create, drop, back up, and restore databases (SQL
  engines only).
- **Change version** - upgrade in place, keeping your data.
- **Uninstall** - remove the engine.

<ThemedImage light="/images/database-configuration-light.png" dark="/images/database-configuration-dark.png" alt="The Configuration dialog for a MySQL database, with copyable .env values" />

<ThemedImage light="/images/manage-databases-light.png" dark="/images/manage-databases-dark.png" alt="The Manage databases dialog, creating and listing MySQL databases" />

## From the command line

### Managing services

```sh
yerd service available            # versions installable for your platform
yerd service install redis 8      # download, install, and start
yerd service install postgres 17-full  # PostGIS build (see below)
yerd services                     # list everything: version, state, port

yerd service start redis        # start it now
yerd service stop redis         # stop for this session (returns on next daemon start)
yerd service restart redis

yerd service set-port redis 6380   # change the loopback port (next start)
yerd service logs redis --lines 50 # tail the service log

yerd service set mysql max_allowed_packet 256M  # engine config directive (next start)
yerd service unset mysql max_allowed_packet     # drop it again
yerd service overrides mysql                    # what's currently set

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
sync. Your own engine directives live alongside it in a `[services.<id>.overrides]`
sub-table; see [Service configuration overrides](#service-configuration-overrides)
below.

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

## Service configuration overrides

Yerd owns each config-backed engine's config file and **regenerates it on every
start** - that is what keeps the port, the data directory, the socket, the log
path, and the loopback-only binding in step with the rest of Yerd. The cost used
to be that any directive you added to that file by hand vanished the next time
the service started.

It doesn't any more. Every engine that has a config file now reads two extra
files from a `conf.d/` directory beside it, and both are read **after** Yerd's own
settings:

```text
<state>/services/mysql/
├─ mysql.conf                # generated by Yerd on every start - don't edit
└─ conf.d/
   ├─ 10-yerd.cnf            # your `yerd service set` overrides - regenerated on every start
   └─ 50-local.cnf           # yours - created once, never rewritten
```

`<state>` is `~/Library/Application Support/io.yerd.Yerd` on macOS and
`~/.local/state/yerd` on Linux. The extension is `.cnf` for MySQL/MariaDB and
`.conf` for PostgreSQL and Redis, because MySQL's include directive reads only
`*.cnf`.

The generated config ends with the engine's own native include: `!includedir` for
MySQL/MariaDB, `include_dir` for PostgreSQL, and - since Redis/Valkey has no
directory form - two explicit `include` lines naming both files in order. Every
one of those reads its files last and in name order, which is where the
precedence comes from.

**Precedence:** `50-local` beats `10-yerd`, which beats Yerd's own defaults.

::: info Which engines support this
`mysql`, `mariadb`, `postgres`, and `redis`. Meilisearch takes its settings from
command-line flags and environment variables rather than a config file, so it
refuses with `does not support configuration overrides` (the same goes for
per-site app servers such as Reverb). In the desktop app the **Override
settings** item simply doesn't appear for them.
:::

### Setting an override

The common case - matching the settings of the production MySQL server your app
actually runs against:

```sh
yerd service set mysql max_allowed_packet 256M
yerd service set mysql max_connections 500
yerd service set mysql sql_mode STRICT_TRANS_TABLES,NO_ZERO_DATE

yerd service overrides mysql
#   max_allowed_packet = 256M
#   max_connections = 500
#   sql_mode = STRICT_TRANS_TABLES,NO_ZERO_DATE

yerd service restart mysql
```

`yerd service unset mysql sql_mode` drops one again. Each override is stored in
`yerd.toml` (so it survives a restart, an upgrade, and a `change-version`) and
rendered into `10-yerd.cnf` on every start. The desktop app has the same thing
under **Override settings** in a service's `⋯` menu.

::: warning Restart to apply
Setting an override never restarts anything, exactly like
`yerd service set-port`. The engine picks it up the next time it starts - run
`yerd service restart <service>` when you're ready.
:::

### Which file should I edit?

| What you want | Where it goes |
|---|---|
| A directive with a name and a value - `max_connections`, `maxmemory`, `work_mem` | `yerd service set`, which renders it into `10-yerd.<ext>` |
| Comments, grouped stanzas, long blocks, or anything you want to keep formatted your way | your editor, in `conf.d/50-local.<ext>` |
| The port, data directory, socket, pid file, log path, or the loopback binding | nowhere - Yerd manages these (use `yerd service set-port` for the port) |

`50-local.<ext>` is created once, as an all-comments stub explaining the rules,
and **never rewritten** - it is yours, so what you put there survives every
restart. It is read last, so it also wins over anything set with
`yerd service set`. If you delete it, Yerd recreates the stub on the next start.

Directives Yerd manages itself are rejected when you set them through
`yerd service set` or the desktop app, with a hint naming the command that does
own them:

```sh
yerd service set mysql port 3307
# error: port is managed by Yerd: the port is managed with `yerd service set-port <service>`
```

The check folds letter case in every engine, and `-` against `_` for
MySQL/MariaDB, so `Bind_Address` is caught just as `bind-address` is - the engines
themselves are that lenient, and an override that slipped through could unpin the
loopback-only binding.

`50-local.<ext>` is a different matter. Yerd never rewrites that file and the
engine reads it directly, so nothing can *refuse* what you put there: a reserved
directive in it does take effect. `yerd doctor` is the safety net, reporting each
one with the file, the line, and the same hint:

```sh
yerd doctor
# ⚠ Service override needs attention
#     …/services/mysql/conf.d/50-local.cnf line 20: bind-address - this directive
#     is managed by Yerd: Yerd pins this service to loopback
```

Run it after hand-editing. It is a warning, not a block, so a directive that
unpins the loopback binding will stay in effect until you remove it and restart.

::: tip Some directives accumulate rather than replace
Last-wins holds for ordinary scalar directives. A few are additive: MySQL's
`plugin-load-add` and Redis' `save` / `client-output-buffer-limit` **append** to
Yerd's value instead of replacing it. And in PostgreSQL, anything written by
`ALTER SYSTEM` lands in `postgresql.auto.conf`, which is read after every include
and therefore outranks both files. The stub comments in `50-local.<ext>` repeat
the caveat for the engine you're looking at.
:::

### Getting a directive wrong

Yerd checks the **shape** of a name and value - a name starts with a letter or
`_` and uses only letters, digits, `.`, `_`, `-`; a value carries no control
characters, `;` or `#`, and stays under 512 bytes. It does not check the meaning:
Yerd holds no table of every directive of every engine, so a well-formed
directive the engine doesn't recognise is accepted here and rejected there.

Most engines refuse to start at all on an unknown option, and they say so while
parsing the file. So if a restart fails after an override, the error carries the
tail of the engine's own log and the path of the hand-edit file:

```sh
yerd service restart mysql
# error: mysql crashed repeatedly (last exit: exit status 1)
# last lines of the service log:
# [ERROR] [MY-000067] unknown variable 'max_allowd_packet=256M'
# check ~/Library/Application Support/io.yerd.Yerd/services/mysql/conf.d/50-local.cnf and `yerd service logs mysql`
```

The message always names the hand-edit file, since that is the one Yerd can't
inspect for you. If the offending line came from `yerd service set` instead,
`yerd service overrides mysql` lists what's set; drop it with
`yerd service unset` and restart again.

`yerd doctor` also reads `50-local.<ext>` for every override-capable engine and
raises a warning per line that names a directive Yerd manages or that isn't a
directive at all, with the file and line number. It never edits the file - that
one is yours, which is the whole point of it.

## PostgreSQL: base and PostGIS (`full`) builds

PostgreSQL ships in **two flavours**, and you choose which one you install by its
version label. `full` is the PostGIS variant, appended to the version as a
`<version>-full` label:

| Label | What you get | Compressed size | License |
|---|---|---|---|
| `17` | The lean base build. | ≈ 6.5 MB | 100% PostgreSQL License (permissive) |
| `17-full` | The base plus **PostGIS** and its geospatial stack. | ≈ 60-64 MB | GPL-encumbered (see below) |

```sh
yerd service install postgres 17        # lean base
yerd service install postgres 17-full   # PostGIS build
```

Both show up as distinct installable versions in `yerd service available` and in
the desktop app's version picker. The build is downloaded once per install and
cached.

### What each build bundles

Both builds ship the standard contrib extensions - `pg_stat_statements`,
`pg_trgm`, `citext`, `unaccent`, `hstore`, `ltree`, `btree_gin`, `btree_gist`,
`fuzzystrmatch`, `tablefunc`, `intarray`, `cube`, `earthdistance`,
`postgres_fdw`, `dblink`, `pageinspect`, `amcheck`, `pgstattuple`,
`pg_buffercache` - plus **`pgvector`** (Linux and macOS).

The **`full`** build adds the geospatial and crypto stack on top:

- **PostGIS** with raster and topology: `postgis`, `postgis_raster`,
  `postgis_topology`, `postgis_tiger_geocoder`, `address_standardizer`.
- `pgcrypto`, `uuid-ossp`, `sslinfo`, and `xml2`.

### Base and `full` share one datadir - switch between them freely

`17` and `17-full` are separate *installs* but **share a single data directory**
(Postgres datadirs are pinned to the major version, and the `full` variant maps to
the same major). So you can install the base build, create databases, then switch
with `yerd service change-version postgres 17-full` (or back) **without losing your
data** - the databases carry across the switch.

One caveat: PostGIS objects created while running `full` need the PostGIS `.so` to
be *used*. The base build still starts against the shared datadir and your regular
tables are fine, but queries that touch PostGIS types or functions only work while
`full` is running - so enable `full` before you start using PostGIS.

Uninstalling with `--purge` deletes the shared datadir, so `yerd service uninstall
postgres <label> --purge` removes the databases for **both** flavours of that major.

::: warning `full` is GPL-encumbered
The base build stays **100% PostgreSQL License** (permissive). The `full` build
bundles GPL/LGPL components - **PostGIS** is GPLv2 and **GEOS** is LGPL-2.1 (PROJ,
GDAL, json-c, and protobuf-c are permissive). Install `full` only if that
licensing suits your project. Each component's notice ships inside the downloaded
tarball (`LICENSE-postgis`, `LICENSE-geos`, `LICENSE-proj`, `LICENSE-gdal`, and
friends).
:::

Backup and restore are unchanged - both builds ship the same `pg_dump` /
`pg_restore` / `psql` tools, so [`yerd db backup` / `restore`](../reference/cli/db)
work identically. A dump that uses PostGIS objects only restores into a `full`
target, because those objects need the PostGIS `.so`.

## Windows and Redis licensing

[Windows service support is still on the roadmap](../developer/cross-platform)
alongside the rest of the Windows platform work. On macOS and Linux all four
engines run today (subject to a published build for your architecture).

## See also

- [Services CLI reference](../reference/cli/services) and [Databases CLI reference](../reference/cli/db)
- [PHP Versions](./php-versions) - the supervision model services share
- [Configuration Reference](../reference/configuration) - the `[services.<id>]` tables
- [yerd-services](../developer/crates/yerd-services) and [yerd-supervise](../developer/crates/yerd-supervise) - the crates behind this
