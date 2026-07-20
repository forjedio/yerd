# yerd-services

`yerd-services` owns Yerd's **local database and cache services**: installing
prebuilt service binaries, supervising one instance per engine, reporting their
live state, and performing SQL database administration (create / drop / list /
backup / restore). It is the services counterpart to [`yerd-php`](./yerd-php) and
is structured the same way - every *decision* is pure and unit-testable; every
byte of I/O sits behind the [`yerd-supervise`](./yerd-supervise) trait seams.

The four engines it models:

| Service | `id` | Display name | Kind | Default port | Server binary |
|---|---|---|---|---|---|
| Redis | `redis` | `Redis (Valkey)` | Cache | 6379 | `valkey-server` |
| MySQL | `mysql` | `MySQL` | Database | 3306 | `mysqld` |
| MariaDB | `mariadb` | `MariaDB` | Database | 3306 | `mariadbd` |
| PostgreSQL | `postgres` | `PostgreSQL` | Database | 5432 | `postgres` |

::: info Redis is served by Valkey
The "Redis" slot is filled by **Valkey** - the BSD-licensed fork - because Redis
7.4+ is SSPL/RSALv2 and not cleanly redistributable. It stays wire-compatible, so
clients are unaffected. The server binary is `valkey-server`; the user-facing
display name is `Redis (Valkey)`.
:::

::: info Crate metadata
`description`: *Local database / cache service supervision and version management
for Yerd.* `#![forbid(unsafe_code)]`. Internal deps:
[`yerd-platform`](./yerd-platform) (`PlatformDirs`, `PortBinder`),
[`yerd-supervise`](./yerd-supervise) (the trait seams + state machine). External:
`thiserror`, `async-trait`, `serde`, `serde_json`, `tokio`.
:::

See also the [Crates overview](../crates), [`yerd-supervise`](./yerd-supervise),
and the user-facing [Services guide](../../guide/services).

## Module map

```text
src/
├── lib.rs            # re-exports + a compile-time Send+'static guard
├── service.rs        # Service / ServiceKind - pure per-engine metadata
├── database.rs       # pure SQL-admin logic (name validation, quoting, argv)
├── config_render.rs  # pure config rendering (redis.conf / my.cnf / postgresql.conf)
├── release.rs        # pure artifact resolution from the hosted listing
├── version.rs        # version labels + on-disk path layout; discover_installed
├── health.rs         # readiness probes (Redis PING, MySQL/Postgres handshake)
├── manager.rs        # ServiceManager - the I/O driver that runs the state machine
└── error.rs          # ServiceError
```

Pure modules: `service`, `database`, `config_render`, `release`, plus the path
builders in `version` and `error`. I/O lives in `manager` (spawning, fs, ports),
`health` (sockets), and `version::discover_installed`.

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## `service.rs` - pure engine metadata

`Service` (`Redis`, `MySql`, `MariaDb`, `Postgres`) and `ServiceKind` (`Cache`,
`Database`) are compile-time facts about each engine - no I/O. `Service::ALL` is
the canonical 4-element iteration order. Beyond the table at the top of this page,
each `Service` exposes:

| Method | Redis | MySql | MariaDb | Postgres |
|---|---|---|---|---|
| `client_binary()` | `None` | `mysql` | `mariadb` | `psql` |
| `dump_binary()` | `None` | `mysqldump` | `mariadb-dump` | `pg_dump` |
| `needs_init()` | false | true | true | true |
| `init_binary()` | `None` | `mysqld --initialize-insecure` | `mariadb-install-db` | `initdb` |
| `kind()` | `Cache` | `Database` | `Database` | `Database` |
| `datadir_pinned_to_major()` | false | false | false | **true** |

MySQL and MariaDB share port 3306, so only one can run on it at a time
(the config layer allows a per-instance override). PostgreSQL pins its data
directory to its major version (`data-<major>`) and refuses to start against a
datadir written by a different major.

## `database.rs` - the SQL-admin security boundary (pure)

All database administration logic is pure and **constructs SQL that cannot be
injected**. The daemon's [`db_admin`](../binaries/yerdd) edge calls these
functions and passes the result to the engine's client as a single argv element -
never through a shell.

- **`validate_db_name(name)`** - strict allowlist: non-empty, ≤ 63 chars (the
  lowest engine cap), first char an ASCII letter or `_`, the rest
  `[A-Za-z0-9_]`. Returns `DbNameError` (`Empty`, `TooLong`, `BadStart`,
  `BadChar(char)`). This policy applies only to databases Yerd creates.
- **`validate_existing_db_name(name)`** - accepts engine-created names selected
  for drop, backup, or restore, including whitespace, punctuation, Unicode, and
  quoting characters. It rejects only empty strings and embedded NUL characters.
- **`is_system_database(service, name)`** - case-insensitive guard. MySQL/MariaDB:
  `information_schema`, `performance_schema`, `mysql`, `sys`; Postgres: `postgres`,
  `template0`, `template1`. System databases cannot be dropped or restored over.
- **`quote_ident(service, name)`** - backticks for MySQL/MariaDB, double-quotes
  for Postgres (each doubling the quote char).
- **`create_sql` / `drop_sql` / `list_sql`** - per-engine statements. Postgres
  drop uses `DROP DATABASE <ident> WITH (FORCE);` (PG13+); Postgres list queries
  `pg_database WHERE datistemplate = false`. List queries return hexadecimal
  UTF-8 so line breaks and surrounding whitespace in names survive client output.
- **`client_args` / `dump_args` / `restore_args`** - build the argv for the
  interactive client / dump tool. MySQL & MariaDB connect over the local **Unix
  socket** as a passwordless `root`; Postgres connects over **TCP loopback** as
  `postgres`. Restore reuses the *client* binary (replaying SQL on stdin), not the
  dump binary.
- **`parse_db_list(service, stdout)`** - decodes hexadecimal names, filters system
  DBs, sorts, and deduplicates without altering database names.

## `config_render.rs` - pure config rendering

One renderer per engine, all returning text the caller writes to disk (no I/O
here):

- **`render_redis_conf`** - loopback bind, `protected-mode`, `daemonize no`, no
  password (local-dev posture).
- **`render_my_cnf`** - one `[mysqld]` renderer for both MySQL and MariaDB: bind
  `127.0.0.1`, `skip-name-resolve`, a Unix socket, and a pid-file.
- **`render_postgresql_conf`** - `listen_addresses = '127.0.0.1'` and
  **`unix_socket_directories = ''`** (Postgres uses TCP loopback only; the macOS
  `sun_path` limit rules out a socket here), with hba/ident pinned to the datadir.

## `release.rs` - artifact resolution (pure)

Yerd hosts its **own** multi-platform service distribution (the
`forjedio/yerd-services` build matrix) with a `services.json` listing
(`LISTING_SCHEMA = 1`). `SERVICES_BASE_URL` points at that GitHub release. The
pure functions here resolve a `(service, version, os, arch)` request against a
fetched listing body:

| Function | Purpose |
| --- | --- |
| `resolve_from_listing(...)` | Finds the build for an exact version, returning an `Artifact { service, version, url }`. Errors `VersionUnavailable` / `ListingParse` / `UnsupportedListingSchema`. |
| `available_versions(...)` | Every version published for the platform (infallible; empty on parse error). Feeds the GUI dropdown and `yerd service available`. |
| `artifact_url` / `listing_url` / `platform_token` | URL construction. |
| `current_os_arch()` | Resolves the host `(Os, Arch)`; errors on Windows / 32-bit. |

The daemon performs the actual HTTPS download (via the `Downloader` seam) and
hands the bytes back; integrity rests on HTTPS to the host (there is no separate
checksum sidecar for service builds).

## `version.rs` - layout & discovery

`ServiceVersion` is an opaque, validated version label (services don't share PHP's
major.minor shape). Labels may carry a **variant suffix** after a hyphen -
Postgres publishes both a lean base (`17`) and a PostGIS `17-full` under the same
service. The label is treated as opaque end to end: `release.rs` reads each
`version` string straight from the `services.json` listing (it never splits a
filename), so a hyphen in the label is safe by construction. Ordering
compares component-by-component and ranks a **plain** build above a suffixed one
at the same number (`"17.10" > "17.10-full"`), so `full` is never mistaken for the
"latest" build when resolving the newest version. `ServiceVersion::major()` ignores
the variant suffix (it splits on the first `.` or `-`), so a base and any variant of
the same major **share one datadir** (`data-<major>`) - which is what lets a
`change-version` between base and `full` preserve databases. For a variant install,
`manager.rs` also probes the install tree for `proj.db` / `gdalvrt.xsd` and, when
found, exports `PROJ_DATA` / `GDAL_DATA` into the postmaster environment (base
installs get neither) - what lets `PostGIS` `ST_Transform` / raster reprojection
resolve their runtime data. The on-disk layout under `PlatformDirs`:

```text
{data}/services/<id>/<version>/bin/<server_binary>   # the install (per version/label)
{data}/services/<id>/data                  (or data-<major> for Postgres) # shared datadir
{state}/services/<id>/<id>.conf            # rendered config
{state}/services/<id>/<id>.log             # captured stdout/stderr
{runtime}/services/<id>/<id>.sock          # Unix socket (MySQL/MariaDB)
```

`discover_installed(&PlatformDirs)` scans for version directories that contain a
real server binary; the daemon calls it at startup.

## `health.rs` - readiness probes

`ReadinessProbe` (service-aware) and `ServiceProbes` (the production dispatcher)
implement [`yerd-supervise`](./yerd-supervise)'s address-only `HealthProbe`. A
bare TCP accept is not enough during datadir init, so each probe requires a real
protocol response:

- **`RedisProbe`** - sends `PING`, expects `+PONG`.
- **`MySqlProbe`** - reads the initial handshake packet (first byte `0x0a`
  greeting or `0xff` ERR). MariaDB reuses this probe (same wire protocol).
- **`PostgresProbe`** - sends a startup message, accepts an `R` (auth) or `E`
  (error) reply tag.

## `manager.rs` - the `ServiceManager` driver

`ServiceManager<S, C, P>` is generic over the `ProcessSpawner`, `Clock`, and
`ReadinessProbe` seams, and always drives the state machine under
`SupervisorPolicy::database()`. It holds one `Instance` per `Service` in a
`BTreeMap` (deterministic shutdown order).

| Method | Behaviour |
| --- | --- |
| `ensure(service, version, port)` | Idempotent. Fast path returns the cached `Listen` if already running; otherwise runs one-time datadir init (staging dir + atomic rename), pre-flights the port via `PortBinder` (→ `PortInUse`), renders + writes the config, and drives the supervisor to `Running`. |
| `restart` / `stop` | Drive the supervisor through a stop (then re-`ensure` for restart). |
| `shutdown()` | Stops every instance in `BTreeMap` order. |
| `snapshots()` | Read-only `ServiceSnapshot { service, version, state, pid, listen }` per instance. |

`ServiceRunState` is `Running` or `Failed` (the daemon fills in `Stopped` for an
engine with no live instance). Per-engine specifics live in `build_cmd` (Redis
`valkey-server <config>`; MySQL/MariaDB `--defaults-file=<config>`; Postgres
`-D <datadir> -c config_file=<config>`) and `stop_protocol` (Postgres →
`MasterInterrupt`, all others → `GroupTerm`).

## How the daemon wires it in

[`yerdd`](../binaries/yerdd) instantiates
`ServiceManager<TokioProcessSpawner, SystemClock, ServiceProbes>` and exposes the
crate over IPC:

- **`bin/yerdd/src/services.rs`** handles `ListServices`, `AvailableServices`
  (downloads the listing on demand), `InstallService` (download + unpack, then
  start), `ChangeServiceVersion` (install → restart → remove old,
  keeping the datadir), `UninstallService` (optional `--purge` deletes data),
  `StartService` / `StopService` (toggle the status-only `enabled` flag; boot
  auto-start keys on installed versions, not this flag), `RestartService`,
  `SetServicePort`, and `ServiceLogs`. The per-service status reports
  `supports_databases` from `ServiceKind`.
- **`bin/yerdd/src/db_admin.rs`** is the I/O edge for database administration. It
  requires a *running* SQL engine with a client binary present, passes the pure
  SQL as a single argv element (no shell), streams `BackupDatabase` to a temp
  sibling and atomically renames it (never truncating the target), and streams
  `RestoreDatabase` into the client's stdin. Create uses the portable name
  allowlist; drop/backup/restore use exact existing names after empty/NUL checks.
  Engine errors are classified into
  typed `ErrorCode`s (`AlreadyExists` / `NotFound` / `Internal`).

See [IPC Protocol](../ipc-protocol) for the full service/database message set.

## Error model

`ServiceError` (`#[non_exhaustive]`, not `Clone + Eq` - it wraps `io::Error` and
`PlatformError`) pins the failure surface: `Unsupported`, `VersionNotInstalled`,
`DiscoveryIo`, `Init`, `Spawn`, `ConfigWrite`, `HealthCheckTimedOut`,
`PermanentFailure`, `PortInUse`, `Bind`, `Kill`, `VersionUnavailable`,
`UnsupportedPlatform`, `ListingParse`, `UnsupportedListingSchema`,
`Download(DownloadError)`, and `Extract`.

::: tip Engine availability is a distribution question, not a code one
All four engines are implemented end-to-end - supervision, init, config rendering,
health probing, and (for the SQL engines) database administration. Whether a given
engine/version installs depends only on whether a prebuilt build is published in
the hosted `services.json` listing for your platform. An unpublished build surfaces
as `VersionUnavailable` rather than a missing feature.
:::
