---
applyTo: "bin/yerdd/**/*.rs"
---

# yerdd — the daemon

Runs **as the user, unprivileged**, and is the single source of truth for
runtime state. It wires the library crates together and serves the IPC protocol.

**Layer:** thin orchestration only. Behaviour belongs in the crates; this binary
coordinates them.

## Owns (orchestration)

- Load `yerd-config`; build a `SiteRouter` from config + a filesystem scan of
  parked paths (`startup.rs`, `state.rs`).
- Start `yerd-dns`, `yerd-proxy`, `yerd-php`, and apply doctor fix plans; own the
  cert store (`cert_store.rs`) and backend resolution (`backend_resolver.rs`).
- Serve the `yerd-ipc` server transport (`ipc_server.rs`), handle mutations
  (`mutate.rs`), PHP install/update flows (`php_install.rs`, `php_updates.rs`),
  signals, and single-instance enforcement.
- Install the `tracing` subscriber + rolling file appender (`tracing_init.rs`).
  Libraries only emit spans/events; the daemon owns the subscriber.

## Privileged work

- For anything needing root, go through `yerd-platform` → `HelperInvocation` and
  invoke `yerd-helper`. The daemon owns that `Command::new(...)`; it never does
  the privileged effect inline.

## Must not

- Reimplement crate logic. If a handler grows real behaviour, push it into the
  relevant crate with tests and call it from here.
- Run as root, or assume elevation. Unprivileged operation is the default; fall
  back to `8080`/`8443` when the user declines elevation.
- Auto-install PHP. **PHP updates are notify-only**: the periodic checker
  (startup + every 12h) and `list` only *report* newer patches. Installs happen
  solely on an explicit update request. Keep it that way.
- Bypass the configured outbound proxy. Every external HTTP request uses a
  client from `crate::network` (`network::downloader` / `network::client`),
  never `reqwest::Client::builder()` or a new `ReqwestDownloader` directly.
  Every subprocess that reaches the network gets `network::apply_child_env` (or
  `network::apply_plan` with a plan from `network::child_env_plan`). The
  documented exception is cloudflared (tunnel edge traffic and its management
  commands, which are deferred; see docs). Never call the factory while holding
  `state.config`.

## Tests / invariants

- `tests/lifecycle.rs` — bring the daemon up on temp paths + ephemeral ports.
- `tests/no_runtime_deps.rs` — dependency-graph guard.

## Review checklist

- [ ] New behaviour lives in a crate, not in a handler.
- [ ] Privileged work routed through the helper; daemon stays unprivileged.
- [ ] PHP updates remain notify-only; no silent auto-install.
- [ ] External HTTP goes through `network::downloader` / `network::client`;
      network subprocesses get `network::apply_child_env` / `apply_plan`.
- [ ] Daemon remains the single source of truth (no client-side state authority).
