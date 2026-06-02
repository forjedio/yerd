# yerd-supervise

`yerd-supervise` is the **process-agnostic supervision substrate** shared by
[`yerd-php`](./yerd-php) (FPM pools) and [`yerd-services`](./yerd-services)
(database / cache daemons). It holds the parts of process supervision that are
*not* specific to any particular supervised program:

- the **trait seams** the supervisor depends on (`ProcessSpawner`, `ChildHandle`,
  `Clock`, `HealthProbe`, `Downloader`);
- the production tokio-backed implementations of the infrastructure traits
  (`SystemClock`, `TokioProcessSpawner`, `TokioChild`);
- the generic [`Listen`](#the-listen-enum) address (Unix socket vs TCP loopback);
- and the **pure** supervision state machine (`supervisor`).

It depends on **nothing internal** - it sits at the bottom of the crate graph
next to [`yerd-core`](./yerd-core). It was extracted from `yerd-php` once a second
consumer (`yerd-services`) needed the same restart/health/backoff machinery with
*different* timing, so the state machine is parameterised by a per-call
[`SupervisorPolicy`](#supervisorpolicy) rather than baking constants in.

::: info Crate metadata
`description`: *Process-agnostic supervision substrate for Yerd.*
`#![forbid(unsafe_code)]`. No internal `yerd-*` dependencies. External deps:
`thiserror`, `async-trait`, `tokio` (process/net/rt/time/io-util/macros/sync), and
Unix-only `nix` (process-group signalling). The only async runtime is `tokio`
(supervision is intrinsically async).
:::

See also the [Crates overview](../crates), [`yerd-php`](./yerd-php), and
[`yerd-services`](./yerd-services).

## Module map

```text
src/
├── lib.rs          # re-exports + a compile-time Send+'static guard
├── error.rs        # DownloadError, ExitReason, SpawnFailureReason
├── listen.rs       # Listen enum (Unix socket vs TCP loopback)
├── traits.rs       # ProcessSpawner, ChildHandle, Clock, HealthProbe, Downloader
├── supervisor.rs   # the pure state machine (PoolState/Event/Action + SupervisorPolicy)
└── real.rs         # SystemClock, TokioProcessSpawner, TokioChild (prod impls)
```

`supervisor`, `listen`, and `error` are **pure**; `real` (and the trait impls)
own all I/O.

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## The trait seams

Every effect a supervisor needs is injected so the driving crate can be tested
with fakes - no real process spawns, no real sockets, no real clock. The
*definitions* live here; the production impls live in `real.rs`, while the
program-specific impls (`HealthProbe`, `Downloader`) live in the consuming crate
or in the daemon.

```rust
pub trait ProcessSpawner: Send + Sync + 'static {
    type Child: ChildHandle;
    fn spawn(&self, cmd: std::process::Command) -> Result<Self::Child, io::Error>;
}

#[async_trait]
pub trait ChildHandle: Send + 'static {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> Result<Option<ExitReason>, io::Error>;
    async fn wait(&mut self) -> Result<ExitReason, io::Error>;
    async fn kill(&mut self, signal: KillSignal, protocol: StopProtocol) -> Result<(), io::Error>;
}

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> std::time::Instant;
}

#[async_trait]
pub trait HealthProbe: Send + Sync + 'static {
    async fn probe(&self, listen: &Listen) -> Result<(), io::Error>;
}

#[async_trait]
pub trait Downloader: Send + Sync + 'static {
    async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError>;
}
```

`ProcessSpawner::spawn` takes a `std::process::Command` (not a tokio one) so the
trait itself stays runtime-free; the production impl converts internally.

::: warning Process-group signalling (Unix)
`ChildHandle::kill` takes a [`StopProtocol`](#stopprotocol) so a supervisor can
choose between signalling the **process group** (the default - reaps workers
along with the master) and signalling the master process only. `TokioProcessSpawner`
sets `process_group(0)` at spawn time, so the child's PID is also the
process-group ID. `real::TokioChild::kill` then uses `nix` `killpg` for a group
signal. On Windows both signals collapse to `tokio::process::Child::kill`, and
children are taken down by tokio's `kill_on_drop(true)`.
:::

::: tip The `Downloader` seam keeps `reqwest` out of the libraries
`Downloader` is transport-agnostic - only `async-trait`, no `reqwest`. The real
`reqwest`-backed implementation lives in the daemon (`bin/yerdd`); tests inject a
fake. `DownloadError` carries a flattened message string rather than wrapping a
transport type, so a test fake can construct it without pulling in `reqwest`.
**Checksum verification of the fetched bytes is the caller's job, not the
downloader's.**
:::

## The pure state machine (`supervisor`)

`supervisor` is the heart of the crate: a pure transition function plus the data
types around it. Time enters as `Elapsed(Duration)` rather than `Instant::now()`,
so a test can construct any state without a real clock.

```rust
#[must_use]
pub fn transition(
    state: PoolState,
    event: Event,
    policy: &SupervisorPolicy,
) -> (PoolState, Action)
```

The timing/restart knobs are **not** baked in - they are supplied per call via
`policy`, so an FPM pool (fast to start, cheap to retry) and a database (slow
cold-boot, expensive to retry) drive the same logic with different tuning.

The five pool states:

```rust
pub enum PoolState {
    Stopped,
    Starting { attempts: u32, pid: Option<u32> },
    Running  { pid: u32 },
    Failed   { last_exit: ExitReason, attempts: u32 },
    Stopping { sigkilled: bool },
}
```

The driver feeds back `Event`s (`EnsureRequested`, `SpawnSucceeded`,
`HealthCheckOk`, `HealthCheckTick`, `Crashed`, `StopRequested`, `StopComplete`,
`StopTick`, `BackoffElapsed`) and receives one `Action` to execute (`None`,
`Spawn`, `HealthCheck`, `Backoff { wait }`, `Kill { signal }`,
`EmitError(ErrorTag)`).

### SupervisorPolicy

`SupervisorPolicy` carries the tunable knobs and ships two named profiles:

| Field | `fpm()` | `database()` | Meaning |
| --- | --- | --- | --- |
| `health_check_window` | 5 s | 60 s | Max total time `Starting` may persist before health-check timeout. |
| `backoff_initial` | 100 ms | 250 ms | First retry wait. |
| `backoff_max` | 10 s | 10 s | Cap; exponential doubling saturates here. |
| `max_restart_attempts` | 3 | 3 | Consecutive failures before `PermanentFailure`. |
| `stop_grace` | 2 s | 10 s | Window between SIGTERM and SIGKILL. |

`SupervisorPolicy::fpm()` is used by `yerd-php`; `SupervisorPolicy::database()`
by `yerd-services` (databases cold-boot slowly and are expensive to retry, so the
health window and stop grace are far wider).

`backoff_for(attempts, policy)` computes
`min(backoff_initial * 2^(attempts-1), backoff_max)`, saturating.

### StopProtocol

`StopProtocol` (default `GroupTerm`) selects how a stop is delivered:

- **`GroupTerm`** - SIGTERM to the whole process group (the usual case; reaps the
  master and its workers together). Used by FPM and by Redis/MySQL/MariaDB.
- **`MasterInterrupt`** - SIGINT to the master process only. Used by
  [`yerd-services`](./yerd-services) for PostgreSQL, whose postmaster treats SIGINT
  as a *fast shutdown*; a group signal would be wrong.

### Invariants

Several invariants are encoded directly in the transition table and asserted by
tests:

- A `HealthCheckOk` arriving **before** `SpawnSucceeded` is an out-of-order event
  and is ignored (state unchanged, `Action::None`).
- An operator `EnsureRequested` on a `Failed { MAX }` pool **resets the restart
  budget** to a fresh `Starting { 1, None }`.
- A `StopRequested` from `Failed` short-circuits straight to `Stopped` with no
  kill (there is no live child to signal).
- A catch-all arm maps every unhandled `(state, event)` pair to
  `(state, Action::None)` so the machine never panics.

## The `Listen` enum

A supervised process listens on either a Unix domain socket or a TCP loopback
address. `Listen` is the shared address type:

```rust
pub enum Listen {
    UnixSocket(PathBuf),     // Unix only
    TcpLoopback(SocketAddr), // always valid; required on Windows
}
```

Unlike most Yerd enums it is deliberately **not** `#[non_exhaustive]` -
exhaustive matching on the two cases is intended at every call site.

## Production impls (`real.rs`)

- **`SystemClock`** wraps `Instant::now()`.
- **`TokioProcessSpawner`** converts the std `Command` to a tokio one, sets
  `kill_on_drop(true)` (so a daemon crash takes its children with it), spawns, and
  reads the PID once.
- **`TokioChild`** wraps `tokio::process::Child`; its `try_wait` / `wait`
  translate `ExitStatus` into [`ExitReason`](#error-model) via
  `ExitReason::from_status`. Its `kill` honours both `KillSignal` (`Term` / `Kill`)
  and `StopProtocol`: on Unix it uses `nix` `killpg`/`kill` (group SIGTERM/SIGKILL,
  or master-only SIGINT for `MasterInterrupt`); the Windows path is a Phase-2 TODO
  that collapses to `Child::kill`.

## Error model

The crate owns three small, transport-free error/reason types reused by both
consumers:

- **`DownloadError`** (`#[non_exhaustive]`) - `Transport { url, reason }`, a
  flattened message so fakes need no `reqwest`.
- **`SpawnFailureReason`** (`#[non_exhaustive]`) - `BinaryNotFound`,
  `PermissionDenied`, `WaitFailed`, `Other`, plus `from_kind` to classify an
  `io::ErrorKind` (`NotFound → BinaryNotFound`, `PermissionDenied → PermissionDenied`,
  else `Other`).
- **`ExitReason`** (`#[non_exhaustive]`, `Hash`) - `Code(i32)`, `Signal(i32)`,
  `Unknown`, plus `from_status` (maps a Unix termination signal to `Signal`,
  otherwise the exit code, otherwise `Unknown`).

## Consumers

| Crate | Policy | Stop protocol | HealthProbe impl |
| --- | --- | --- | --- |
| [`yerd-php`](./yerd-php) | `fpm()` | `GroupTerm` | `FastCgiProbe` (FastCGI `GET_VALUES`) |
| [`yerd-services`](./yerd-services) | `database()` | `GroupTerm`, plus `MasterInterrupt` for Postgres | `ServiceProbes` (Redis PING, MySQL/MariaDB handshake, Postgres startup) |

Both crates include a compile-time `Send + 'static` guard over their production
manager instantiation, mirroring the guard in this crate's `lib.rs`.
