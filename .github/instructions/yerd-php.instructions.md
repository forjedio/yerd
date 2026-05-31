---
applyTo: "crates/yerd-php/**/*.rs"
---

# yerd-php — PHP-FPM supervision & versions

Manages PHP-FPM pools per version and the set of installed PHP binaries.

**Layer split (physical):** `pure/` (`fpm_conf`, `supervisor`, `env_scrub`) is
sync and runtime-free; `io/` (`atomic_write`, `fastcgi_probe`) and the
`tokio`-driven manager are the edge. Side effects go through the traits in
`traits.rs`; `real.rs` holds the production trait impls.

## Owns

- Pure FPM config rendering (template → string) and environment scrubbing.
- The supervision **state machine** (spawn per version, health-check, restart on
  crash) expressed against `ProcessSpawner` + `Clock` traits.
- Socket/port allocation via the `Listen` enum, and PHP version discovery /
  release handling.
- Optional install (download + SHA-256 verify) **behind a `Downloader` trait**.

## Must not

- Route requests — that is `yerd-proxy`.
- Hit the network directly for downloads — go through the `Downloader` trait so
  tests stay offline. `reqwest` must not appear in the default-build graph.
- Pull in `anyhow` or any OpenSSL/native-tls variant.

## Conventions & traps

- **No Unix sockets for PHP-FPM on Windows** — use TCP loopback there. Keep this
  abstracted behind the `Listen`/`Backend` enums; never hardcode a socket path.
- Spawn/clock/download are always trait calls in logic; real forks happen only
  in `real.rs` and integration paths. Unit tests use fakes — never real forks.
- FPM config rendering is golden-tested; regenerate the golden only on an
  intended template change.

## Tests / invariants

- `tests/fpm_conf_golden.rs` — exact rendered FPM config.
- `tests/supervisor_states.rs` — state machine via fake spawner + fake clock.
- `tests/no_runtime_deps.rs` — `anyhow`/`reqwest`/OpenSSL absent from the default
  graph; `tokio`/`time` resolve to a single version. `tokio` is allowed here.

## Review checklist

- [ ] New side effect goes through a trait, with a fake-backed test.
- [ ] `Listen`/`Backend` abstraction preserved (no hardcoded Unix socket).
- [ ] No `reqwest`/`anyhow`/OpenSSL in the default graph.
- [ ] Golden FPM config updated only for an intended change.
