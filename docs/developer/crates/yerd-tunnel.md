# yerd-tunnel

`yerd-tunnel` is the **tunnel provider** crate for Cloudflare
(`cloudflared`), ngrok and zrok: it generates each agent's argument vectors and
config, parses their log output, probes `cloudflared`'s edge health, computes
share preflight warnings, and supervises the long-running tunnel children. It
powers the user-facing [Sharing Sites](../../guide/sharing) feature.

The crate is modeled on [`yerd-php`](./yerd-php) (not on the strictly-pure
[`yerd-core`](./yerd-core)): the `origin`, `args`, `parse`, `config`,
`providers` and `preflight` submodules are **pure** (sync, I/O-free, table-tested), while
`manager` and `probe` are the **async I/O edge** that spawns, supervises, and
health-checks the child behind injected traits. This keeps the supervised
driver in a *library* (so the binaries stay thin) while the logic stays
unit-testable.

::: info Crate metadata
`description`: *Tunnel provider (Cloudflare, ngrok, zrok) argument/config
generation and supervised tunnel lifecycle for Yerd.* `#![forbid(unsafe_code)]`. Depends only on
[`yerd-supervise`](./yerd-supervise) (the restart/health state machine and its
`ProcessSpawner` / `ChildHandle` / `Clock` / `HealthProbe` traits),
`async-trait`, and `thiserror`; the only async runtime is `tokio` (`time`,
`macros`, `net`, `io-util`). It does **not** depend on any binary - the
`cloudflared`, `ngrok` and `zrok2` installers (regular tools in `yerdd`'s
[tools subsystem](../dev-tools)), the Cloudflare-account flows, the zrok
enable/disable commands, the provider readiness report, the per-instance driver task, and the tunnel origin
listener live in [`yerdd`](../binaries/yerdd) and [`yerd-proxy`](./yerd-proxy),
which call into this crate.
:::

See also the [Crates overview](../crates), [`yerd-supervise`](./yerd-supervise)
(the shared supervision substrate), and the [Sharing Sites
guide](../../guide/sharing).

## Module map

The `pure` / `io` split: every *decision* (which origin URL, which argv, which
log line is an error, whether a status line is OK, what the config renders to,
which preflight warnings apply) is synchronous and I/O-free; the only *effects*
(spawning, sleeping, reading the logfile, writing the pidfile, killing, the
`/ready` request) sit in `manager` and `probe`.

```text
src/
├── lib.rs        # re-exports + TunnelKind (Quick, Named, Ngrok, Zrok)
├── origin.rs     # origin_url - http://127.0.0.1:<port>, the daemon's tunnel listener
├── args.rs       # cloudflared argv builders (quick_tunnel/named_run/login/create/route/delete/...)
├── config.rs     # render_ingress_config (one catch-all rule) + render_quick_config
├── parse.rs      # log scanners: parse_quick_url / last_error_line / find_auth_url / find_tunnel_id
├── providers.rs  # ngrok/zrok argv + ngrok.yml, URL and error parsers, version gate, redaction
├── preflight.rs  # preflight(&ShareFacts) - share warnings (Vite hot file or no build, SESSION_DOMAIN)
├── probe.rs      # CloudflaredReadyProbe - GET /ready on the metrics server (io)
├── error.rs      # TunnelError
└── manager.rs    # TunnelManager - the async supervised driver (the I/O edge)
```

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## Routing: a dedicated origin, no Host rewrite

Every tunnel, whatever the provider, points its agent at the daemon's **tunnel
origin listener**: a loopback-only, plain-HTTP listener served by
[`yerd-proxy`](./yerd-proxy)'s `serve_tunnel`, at the URL from
`origin_url(port)`. That listener routes by the **public** `Host` through a
daemon-owned `public host -> site` map and presents each request to PHP as
HTTPS on port 443 with the public host. So this crate generates no Host
rewrite, no SNI, and no origin TLS options, and the tunnel doesn't depend on the
80/443 listeners or port-redirect state.

- **Quick** - `quick_tunnel_args(origin, config_path)` passes `--url <origin>`
  plus an explicit `--config` pointing at the ingress-free
  `render_quick_config()` file, so a user or system `cloudflared` config is
  never picked up.
- **Named** - `render_ingress_config(uuid, creds, origin)` emits a single
  catch-all `- service: <origin>` rule. Because the routing decision lives in
  the daemon, exposing or unexposing a site only changes DNS and the route map;
  the `cloudflared` process is never restarted for it.

- **ngrok** - `ngrok_share_args(origin, config_path, static_domain)` runs
  `ngrok http <origin> --config <path>`, adding `--url https://<domain>` for a
  static domain. The `--config` file is Yerd's own `render_ngrok_config()`
  (JSON logs to stdout; no web inspector, update check or console UI), so the user's `ngrok.yml`
  is never read. The authtoken is never in argv: the daemon puts it in the
  child's env as `NGROK_AUTHTOKEN` (`NGROK_AUTHTOKEN_ENV`).
  `ngrok_version_supported` gates `--url` on `NGROK_MIN_VERSION` (3.16.0).
- **zrok** - `zrok_share_args(origin)` runs `zrok2 share public <origin>
  --headless --backend-mode proxy --force-local`, an ephemeral public share. The daemon pins `HOME` to its own zrok environment, which
  `zrok_enable_args` / `zrok_disable_args` set up and tear down.

ngrok and zrok forward the public `Host` unchanged, so the same origin listener
and share passwords apply to them.

The metrics address is not in argv: the manager sets `TUNNEL_METRICS=<addr>` in
a `cloudflared` child's environment on every spawn.

## The supervised driver

`TunnelManager<S: ProcessSpawner, C: Clock>` reuses
[`yerd-supervise`](./yerd-supervise)'s pure `transition` state machine and the
`SupervisorPolicy::tunnel()` profile: a 60s readiness window for a cold edge
connect, a 5s stop grace, backoff from 1s capped at 30s, and **no restart
limit** (`max_restart_attempts = u32::MAX`), so an outage of hours simply keeps
cycling. The child is stored generically as `S::Child` so a `FakeSpawner`
substitutes in tests.

Supervision is **tick-based**: `begin()` registers and spawns the child, then a
driver (a per-instance task in `yerdd`) calls `advance(key, generation)`,
**re-acquiring the manager lock per tick and sleeping with the lock released**.
Every lock-hold is bounded (an FSM step plus a log read, or one stop grace), and
no network I/O ever happens under the lock:

- While **starting**, `advance` checks the child and, for every kind but the
  named tunnel, scans the log for the public URL: `parse_quick_url` for a quick
  tunnel, `parse_ngrok_url` for ngrok (the `url` of a log record whose host is
  the static domain or an ngrok domain) and `parse_zrok_url` for zrok (the URLs after
  its headless banner). When a health check is due it
  returns `Step::Probe(addr)` without probing.
- The driver runs `CloudflaredReadyProbe` against that address with no lock held
  (a `GET /ready` with a 500ms total timeout; `cloudflared` answers `200` while an
  edge connection is registered), then reports the outcome with
  `record_probe(key, generation, ok)`.
- A Cloudflare tunnel becomes ready once the probe is OK (and, for quick, the
  URL is known). ngrok and zrok expose no health endpoint, so they are ready as
  soon as their URL is logged (`TunnelKind::uses_ready_probe` /
  `needs_url`). `Step::Ready` is returned only on that transition.
- While **running**, every tick checks the child and probes again every
  `MONITOR_INTERVAL` (2s). A failed probe marks the edge down without a respawn;
  an exit feeds `Crashed` to the FSM (Failed, then a backoff respawn) and records
  `last_error` from the exit reason plus the log's last error in that agent's
  format (`parse::last_error_line`, `last_ngrok_error`, `last_zrok_error`).

For an ngrok instance the authtoken from its env is redacted (`redact`) from
`last_error`, and its logfile is rewritten without the token on a crash, a
readiness timeout and a stop. A logfile over 1 MiB is removed rather than
rewritten.

`snapshots()` reports a `TunnelState` through a pure `report_state(PoolState,
ever_ready, edge_up)`:

| `TunnelState` | When |
| --- | --- |
| `Connecting` | Starting, and the edge was never reached. |
| `Running` | FSM running and the last `/ready` was OK. |
| `Reconnecting` | Running with the edge down, or restarting after having been online. |
| `Failed` | The process exited (or missed its readiness window); a respawn is pending. |

Each spawn also writes the child's PID to the `LaunchSpec.pidfile` (removed on
stop), so `yerdd` can reap an orphan left by a crashed daemon before it starts
new tunnels. A spawn error or a metrics-address error is recorded as
`last_error` and handled as a crash, not returned as an `Err`.

### Generation fence

Every `begin` that creates an instance draws a fresh number from a manager-wide
monotonic counter and returns it as `Ok(Some(generation))`. `advance`,
`record_probe`, and `stop_if_generation` all take that generation:

- A driver whose generation no longer matches (its instance was stopped, or
  stopped and begun again) gets `Step::Gone` and exits; its late probe results
  are discarded.
- `stop_if_generation` only stops the instance it started, so a failed first
  quick start can't stop a replacement begun by someone else.
- `begin` returns `Ok(None)` while a live **or in-progress** instance exists
  (including one in `Failed` waiting out its backoff). Exposing another site,
  `publish`, or re-sharing therefore never restarts a running connector, which
  is the regression guard for #225.
- After `shutdown()`, `begin` returns `TunnelError::ShuttingDown`, so a late
  resume or ensure can never leave an unsupervised `cloudflared` behind.

## Preflight

`preflight(&ShareFacts { vite_hot, vite_unbuilt, dotenv, tld })` returns
human-readable, non-blocking warnings, in a stable order: Vite's `hot` file in
the served root (`VITE_HOT_WARNING`), otherwise a Laravel site (an `artisan`
file) with a `vite.config.*` but no `build/manifest.json` in the served root
(`VITE_UNBUILT_WARNING`), and a `SESSION_DOMAIN` in `.env` equal to or under the local TLD (handles
`export `, quotes, inline comments, and blank/`null` as unset). The daemon
gathers the facts when a share starts and caches the result per site.

## Public API

Re-exported from `lib.rs`:

```rust
pub use error::TunnelError;
pub use manager::{
    LaunchSpec, MetricsAddrSource, Step, TunnelManager, TunnelSnapshot, TunnelState,
};
pub use origin::origin_url;
pub use preflight::{preflight, ShareFacts};
pub use probe::CloudflaredReadyProbe;
```

| Item | Layer | Role |
|------|-------|------|
| `origin_url(port)` | pure | The loopback tunnel-listener URL every tunnel points at. |
| `args::*` | pure | One argv builder per `cloudflared` invocation. |
| `config::render_ingress_config(...)` / `render_quick_config()` | pure | The named catch-all `config.yml` and the ingress-free quick config. |
| `parse::*` | pure | Log scanners (quick URL, last `ERR` line, auth URL, tunnel id). |
| `providers::*` | pure | ngrok and zrok argv, `render_ngrok_config`, URL and error parsers, version parsing and the ngrok minimum, token redaction. |
| `preflight(...)` / `ShareFacts` | pure | Share warnings from facts the daemon gathers. |
| `probe::is_ok_status_line` | pure | Whether a status line is `HTTP/1.x 200`. |
| `CloudflaredReadyProbe` | io | The `/ready` health probe (`yerd_supervise::HealthProbe`). |
| `LaunchSpec` | - | Binary, args, pinned env, logfile, per-spawn metrics source, pidfile. |
| `TunnelManager` | io | Supervise one tunnel child per key (`<site>`, `@named`, `<site>@ngrok`, `<site>@zrok`) via the shared FSM, generation-fenced. |
| `TunnelSnapshot` / `TunnelState` | - | Reported health, URL/hostname, generation, and `last_error`. |
| `TunnelKind` | - | `Quick`, `Named`, `Ngrok { static_domain }` or `Zrok`. |
| `TunnelError` | - | The crate's error type (`thiserror`), including `ShuttingDown`. |
