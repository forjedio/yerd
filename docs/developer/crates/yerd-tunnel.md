# yerd-tunnel

`yerd-tunnel` is the **Cloudflare Tunnel** support crate: it generates the
`cloudflared` argument vectors and `config.yml`, parses `cloudflared`'s log
output for readiness, and supervises the long-running `cloudflared` child. It
powers the user-facing [Sharing Sites](../../guide/sharing) feature.

The crate is modeled on [`yerd-php`](./yerd-php) (not on the strictly-pure
[`yerd-core`](./yerd-core)): the `origin`, `args`, `parse`, and `config`
submodules are **pure** (sync, I/O-free, table-tested), while `manager` is the
**async I/O edge** that spawns and supervises the child behind injected traits.
This keeps the supervised driver in a *library* (so the binaries stay thin) while
the logic stays unit-testable.

::: info Crate metadata
`description`: *Cloudflare Tunnel argument/config generation and supervised
cloudflared lifecycle for Yerd.* `#![forbid(unsafe_code)]`. Depends only on
[`yerd-supervise`](./yerd-supervise) (the restart/health state machine and its
`ProcessSpawner` / `ChildHandle` / `Clock` traits) and `thiserror`; the only
async runtime is `tokio` (`time` + `macros`). It does **not** depend on any
binary - the `cloudflared` installer and the Cloudflare-account flows live in
[`yerdd`](../binaries/yerdd), which calls into this crate's pure helpers and
drives its `TunnelManager`.
:::

See also the [Crates overview](../crates), [`yerd-supervise`](./yerd-supervise)
(the shared supervision substrate), and the [Sharing Sites
guide](../../guide/sharing).

## Module map

The `pure` / `io` split: every *decision* (which loopback target, which argv,
whether the tunnel is ready, what the config renders to) is synchronous and
I/O-free; the only *effects* (spawning, sleeping, reading the logfile, killing)
sit in `manager`.

```text
src/
├── lib.rs        # re-exports + TunnelKind
├── origin.rs     # OriginTarget::for_site - loopback scheme/port + Host-rewrite flags
├── args.rs       # cloudflared argv builders (quick_tunnel/named_run/login/create/route/delete/...)
├── config.rs     # render_ingress_config - the named tunnel's config.yml (one rule per site)
├── parse.rs      # log scanners: parse_quick_url / is_named_ready / find_auth_url / find_tunnel_id
├── error.rs      # TunnelError
└── manager.rs    # TunnelManager - the async supervised driver (the I/O edge)
```

[Browse the source on GitHub.](https://github.com/forjedio/yerd)

## Routing: a Host-header rewrite, not a proxy change

A tunnel doesn't add any seam to the reverse proxy. Yerd's proxy already routes
purely by the `Host` header matching `.test`, so `yerd-tunnel` has `cloudflared`
**rewrite the Host header** to the site's canonical `{name}.{tld}` and point it at
Yerd's own loopback listener. `OriginTarget::for_site` encodes the choice:

- **Secure site** → `https://127.0.0.1:{https_bound}` with `--http-host-header`,
  `--origin-server-name` (the SNI drives the proxy's per-site cert resolver), and
  `--no-tls-verify` (the loopback hop uses Yerd's private CA, so public-trust
  validation is skipped - intentionally).
- **Non-secure site** → `http://127.0.0.1:{http_bound}` with `--http-host-header`
  (secure sites must take the HTTPS path to avoid the proxy's HTTP→HTTPS redirect).

The named tunnel serves every exposed site from one process: `render_ingress_config`
emits one ingress rule per site (each with its own origin) plus the mandatory
terminal `service: http_status:404`.

## The supervised driver

`TunnelManager<S: ProcessSpawner, C: Clock>` reuses
[`yerd-supervise`](./yerd-supervise)'s pure `transition` state machine and the
`SupervisorPolicy::tunnel()` profile (a generous 60s readiness window for a cold
edge connect, a 5s stop grace). The child is stored generically as `S::Child` so
a `FakeSpawner` substitutes in tests.

Supervision is **tick-based**: `begin()` registers and spawns the child, then the
caller drives readiness with `advance()`, **re-acquiring the manager lock per tick
and sleeping with the lock released**. Every lock-hold is therefore bounded (a sync
FSM step, or one stop-grace window on a kill), so `stop`/`status`/shutdown stay
responsive and a stuck connect can be cancelled. Readiness comes from the child's
logfile: the assigned `*.trycloudflare.com` URL (quick) or an edge-registration
line (named), parsed by the `parse` submodule.

## Public API

Re-exported from `lib.rs`:

```rust
pub use config::IngressRule;
pub use error::TunnelError;
pub use manager::{LaunchSpec, Step, TunnelManager, TunnelSnapshot, TunnelState};
pub use origin::{OriginTarget, Scheme};
```

| Item | Layer | Role |
|------|-------|------|
| `OriginTarget::for_site(...)` | pure | The loopback scheme/port/Host-rewrite flags for a site. |
| `args::*` | pure | One argv builder per `cloudflared` invocation. |
| `config::render_ingress_config(...)` | pure | Render the named tunnel's `config.yml`. |
| `parse::*` | pure | Log scanners (URL capture, readiness markers, auth URL, tunnel id). |
| `LaunchSpec` | - | Binary + args + pinned env + logfile for one spawn. |
| `TunnelManager` | io | Supervise one `cloudflared` child per site via the shared FSM. |
| `TunnelKind` | - | `Quick` vs `Named`. |
| `TunnelError` | - | The crate's error type (`thiserror`). |
