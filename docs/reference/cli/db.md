# Databases

The `yerd db` commands manage the databases *inside* a running SQL service. They
apply to the SQL engines only - `mysql`, `mariadb`, and `postgres`. (Redis is a
cache and has no SQL databases.) To install, start, or stop the engines
themselves, see [Services](./services).

::: tip The engine must be running
Every `yerd db` command connects to a live server, so the target service must be
installed and started first (`yerd service start <svc>`). Database operations need
no elevation - they connect over the local socket / loopback as a passwordless
local-dev superuser.
:::

## Commands

| Command | Description |
| --- | --- |
| `yerd db list <SVC>` | List the databases in the service (system databases are filtered out). |
| `yerd db create <SVC> <NAME>` | Create a new database. |
| `yerd db drop <SVC> <NAME>` | Drop a database (irreversible). |
| `yerd db backup <SVC> <NAME> <FILE>` | Dump a database to a plain-SQL file. |
| `yerd db restore <SVC> <NAME> <FILE>` | Restore a database from a plain-SQL file. |

```sh
yerd db create mysql my_app
yerd db list mysql
yerd db backup mysql my_app ./my_app.sql
yerd db restore mysql my_app ./my_app.sql
yerd db drop mysql my_app
```

## Database names

`create` and the other commands validate the database name client-side before
connecting, and the daemon validates it again before building any SQL. A name
must:

- be non-empty and at most **63 characters** (the lowest limit across the engines);
- start with an ASCII letter or underscore (`_`);
- contain only letters, digits, and underscores.

This strict allowlist is a deliberate security boundary: because the name can
never contain quoting or statement separators, the generated SQL is injection-proof
by construction.

::: warning System databases are protected
Engine-internal databases (`mysql`, `information_schema`, `performance_schema`,
`sys`, `postgres`, `template0`, `template1`) are filtered from `db list` and
cannot be dropped or restored over.
:::

## Backup & restore

`backup` runs the engine's dump tool (`mysqldump`, `mariadb-dump`, or `pg_dump`)
and writes plain SQL. The output is streamed to a temporary file next to the
destination and atomically renamed into place, so a failed dump never truncates an
existing file. A relative `<FILE>` resolves against the current directory.

`restore` replays a plain-SQL file back into an existing database through the
engine's interactive client (`mysql`, `mariadb`, or `psql`) - so the target
database must already exist (`yerd db create` it first if needed).

```sh
# round-trip a database
yerd db create postgres shop
yerd db restore postgres shop ./shop.sql
yerd db backup postgres shop ./shop-$(date +%F).sql
```

## See also

- [Services](./services) - installing and supervising the engines
- [Services & Databases guide](../../guide/services) - the full model
- [yerd-services](../../developer/crates/yerd-services) - `database.rs`, the pure SQL-admin boundary
