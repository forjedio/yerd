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
- **No PHP-FPM binary on Windows at all.** FPM is a Unix-only SAPI; the published
  Windows bundle ships `php-cgi.exe` (single-threaded NTS `FastCGI` server), not
  `php-fpm.exe`. So `build_cmd` is cfg-split: Unix spawns `php-fpm --fpm-config
  <conf>` in its own process group; Windows spawns `php-cgi.exe -b 127.0.0.1:<port>`
  with `PHP_FCGI_MAX_REQUESTS=0` (else php-cgi exits after N requests and the
  supervisor counts it as a crash). Per-pool ini settings/directives/CA go into a
  supplemental ini loaded via `PHP_INI_SCAN_DIR` (never `-c`, which would drop the
  bundle's own `php.ini`); the `fpm_conf` template is not rendered on Windows.
  A `php-cgi.exe` serves one request at a time (no pre-fork on Windows), so a
  version is backed by `WORKERS_PER_VERSION` of them (4 on Windows, 1 elsewhere),
  each on its own loopback port. `PhpManager::ensure` spawns them lazily and
  hands their addresses out round-robin, one worker per call. The count is a
  **fixed compile-time constant**, deliberately with no `yerd.toml`, IPC or GUI
  surface; `snapshots()` aggregates the workers back to one row per version.
  A `Pool` is inserted only once a worker has started, so "installed but never
  started" stays unrepresented and the daemon still renders it as `Stopped`.
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
