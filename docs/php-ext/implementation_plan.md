# yerd-php-ext — Implementation Plan

> **Handoff seed.** Copy to the new `yerd-php-ext` repo. This is the phased build plan
> for the extension itself. See `architecture.md` (design) and `CLAUDE.md` (operating
> guide). The plan is ordered to de-risk the unknowns first and to produce a testable
> artifact for Yerd as early as possible.

## Goal & definition of done

A native PHP extension (`.so`) that, with zero app changes, emits the telemetry frames
in `architecture.md` §2 to Yerd's loopback dump server — built and released per
`(PHP minor × {macOS arm64/x86_64, linux x86_64/aarch64} × NTS, glibc on Linux)`, which
Yerd downloads and loads via `-d zend_extension`.

## Phase 0 — Spike: prove the engine path (highest-risk first)

Smallest possible extension that proves every risky mechanism end-to-end:

1. **Scaffold** an `ext-php-rs` `cdylib` with the `observer` feature; build a `.so`.
2. **Register the INI directive** `yerd_dump.state_path` (`PHP_INI_SYSTEM`) in MINIT and
   read it back at RINIT (verify a `-d yerd_dump.state_path=…` value is visible — an
   *unregistered* name is not).
3. **Observe one symbol** (start with the global `dump`/`dd` or `VarDumper::dump` —
   **pin the exact choice here** and record it in `architecture.md` §3.2). In the
   handler, read `$this`/args from `ExecuteData` and render the dumped value.
4. **Emit one frame**: read `state.json`, open a non-blocking loopback socket to the
   configured port, write one newline-JSON `dump` frame. Verify with `nc -l 2304` and
   then against Yerd's real dump server.
5. **Prove `dlopen`** on the targets that matter: load the `.so` into a **glibc-Linux**
   PHP-FPM (x86_64) and a **macOS** PHP via `-d zend_extension=…`, and serve a request
   without crashing. (musl static PHP cannot load shared extensions — that's why Yerd
   switches Linux to glibc; you only ever target glibc/macOS.)
6. **Panic safety harness**: force a panic inside the observer and confirm it's caught
   and the request still completes.

**Exit criteria:** a frame from a real `dump()` call appears in Yerd's Dumps window on
both macOS and glibc-Linux, and a deliberate observer panic does not crash the worker.

## Phase A — Dumps + queries (highest value, framework-agnostic)

1. **Dumps**: finalize the dump symbol + caller `file:line` resolution; render HTML +
   text; preserve normal user-visible output; handle `dd()` (emit before `exit`).
2. **Queries**: observe `PDOStatement::execute` / `PDO::exec` / `PDO::query`; capture
   SQL, bound params, duration (time begin→end), caller `file:line`. Works for any PHP
   app, not just Laravel.
3. **State/feature gating**: honor `enabled` + `features.dumps` / `features.queries`;
   fast no-op when off.
4. **Frame plumbing**: shared, tested serializer for the §2.2 schema; per-request
   `request_id`; truncation caps.
5. **Robustness**: connect-once-per-request, non-blocking, swallow all errors.

**Exit criteria:** dumps + queries stream correctly under load; disabling each feature
silences it; server-down is invisible to the app.

## Phase B — Laravel signals: jobs, views, requests, logs, cache

1. **Request summary** at RINIT/RSHUTDOWN from superglobals (method/uri/status/duration).
2. **Events**: observe `Illuminate\Events\Dispatcher::dispatch` (+ the logger); filter
   event classes for jobs (`JobProcessing/Processed/Failed`), views (`composing:`/
   `creating:`), cache (`CacheHit/Missed/KeyWritten/KeyForgotten`), logs. Map each to
   its payload (§2.2).
3. **Graceful absence**: non-Laravel apps simply get dumps + queries + request; the
   Laravel observers no-op when the classes aren't present.

**Exit criteria:** all categories visible in Yerd's tabs against a real Laravel app.

## Phase C — Build matrix & releases (CI)

1. **Per-target builds** against the matching PHP headers (the same static-php.dev
   builds Yerd ships: glibc/NTS on Linux, NTS on macOS) so build-ids match. Cells:
   PHP minor × {macos-arm64, macos-x86_64, linux-x86_64, linux-aarch64}.
2. **Artifact naming** `yerd-dump-<phpminor>-<os>-<arch>.so`; publish to GitHub
   Releases (the channel Yerd downloads from).
3. **Release automation**: tag → matrix build → upload assets; a manifest/checksums
   file Yerd can verify.
4. **New-PHP-minor process**: documented steps to add a minor (it needs a fresh build;
   `ZEND_MODULE_API_NO` is per-minor).

**Exit criteria:** `yerd-dump-*` assets exist for the current supported PHP minors on
all four targets, downloadable by Yerd.

## Phase D — Testing & hardening

- **Per-minor smoke tests**: load the `.so`, run `dump`/`dd`/`ddd`, a PDO query, a
  Laravel job/view/cache/log, assert the expected frames (capture with a tiny test TCP
  sink).
- **Negative tests**: server down, missing/garbage `state.json`, feature toggles,
  deliberate observer panic, huge payloads (truncation).
- **Real-app matrix**: a sample Laravel app per supported version.
- **Perf**: confirm the disabled fast-path and the `should_observe` filtering keep
  overhead negligible on hot paths.

## Coordination with Yerd

The contract in `architecture.md` §2 is the seam. When Yerd is ready it will:
- write `state.json` and pass `-d yerd_dump.state_path=…` + `-d zend_extension=…`,
- run the dump server on the configured loopback port,
- download `yerd-dump-<minor>-<os>-<arch>.so` from this repo's releases.

Any schema/INI/port/naming change must be coordinated in both repos.

## Risks (extension-side)
1. **Panic/segfault in-request** kills the FPM worker → strict panic-catching + the
   robustness rules are P0.
2. **ABI/build-id mismatch** → build against Yerd's exact PHP; gate per-minor.
3. **Observer data extraction edge cases** (large/cyclic objects, binary bindings) →
   bounded, panic-safe `Zval` rendering.
4. **`dd()` before `exit`** → emit synchronously in the observer; verify output is
   preserved.
5. **Build-matrix maintenance** → automate; document the add-a-minor steps.
