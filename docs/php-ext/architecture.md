# yerd-php-ext — Architecture

> **Shipped integration.** This document describes how Yerd integrates the native
> `yerd-dump` PHP extension - the design of the extension and the contract between
> it and Yerd. The extension lives in its own repository,
> [`forjedio/yerd-php-ext`](https://github.com/forjedio/yerd-php-ext), and ships its
> builds via GitHub Releases; Yerd downloads and loads them. The two sides are coupled
> only through the integration contract in §2 - any change there must land in both
> repos.

## 1. What this is

`yerd-php-ext` is a regular PHP **`extension`** (not a `zend_extension`; written in **Rust** via
[`ext-php-rs`](https://ext-php.rs)) that does what Laravel Herd's proprietary
extension does: with **zero changes to the user's application**, it intercepts
`dump()`/`dd()` and observes Eloquent queries, dispatched jobs, Blade views, HTTP
requests, log writes, and cache events, then streams each as a small JSON frame to a
loopback TCP server inside the Yerd daemon (`yerdd`), which renders them in a GUI
"Dumps" window.

It exists because a **pure-PHP** approach can't do this reliably: an
`auto_prepend_file` runs *before* `public/index.php`, so the Laravel container
doesn't exist yet, and PHP exposes no global pre-boot hook to register event
listeners from outside the app. A native extension using the **`zend_observer` API**
(PHP 8.0+) sidesteps that entirely — it hooks function/method execution at the engine
level, so it can observe the app the moment it boots (and observe `PDO` queries even
in non-Laravel apps).

**It is consumed by Yerd, not installed by end users.** Yerd downloads the matching
`.so` per installed PHP version and loads it into PHP-FPM via
`php-fpm -d extension=<path>`. Users never run `composer require` or `pecl install`.

## 2. The integration contract (the seam with Yerd — keep in sync)

This is the **only** coupling between the two repos. Any change here must land in
both. Yerd's side is the dump server + `state.json` writer + `-d` flags.

### 2.1 Transport
- **Loopback TCP only**: connect to `127.0.0.1:<port>`. Never bind/connect to anything
  non-loopback.
- **Newline-delimited JSON**: one compact UTF-8 JSON object per line, terminated by
  `\n`. One frame = one line. No length prefix.
- The extension is a **client**; `yerdd` is the server. The extension opens a
  connection per request (or reuses a per-request connection), writes frames, and
  closes at request end.

### 2.2 Frame schema
Every frame is:

```jsonc
{
  "category": "dump|query|job|view|request|log|cache|http",
  "ts": 1718360452123,          // epoch milliseconds (integer)
  "site": "blog.test",          // SERVER_NAME / HTTP_HOST, best-effort ""
  "request_id": "9f2c1a…",      // stable per PHP request; generated at RINIT
  "payload": { … }              // category-specific, see below
}
```

`request_id` lets the GUI **group rows by request** (the dividers in the screenshot).
Generate it once at RINIT (random hex, ~16 bytes) and reuse for every frame in that
request.

**Per-category `payload`:**

| category  | payload fields |
|-----------|----------------|
| `dump`    | `value_html` (rendered dump HTML), `value_text` (plain fallback), `file`, `line` |
| `query`   | `sql`, `bindings` (array), `time_ms` (float), `connection` (string), `file`, `line` |
| `job`     | `name`, `connection`, `queue`, `status` (`processing\|processed\|failed`), `time_ms`, `exception?` |
| `view`    | `name`, `path`, `data_keys` (array of bound variable names) |
| `request` | `method`, `uri`, `status` (int), `duration_ms` (float), `ip` |
| `log`     | `level`, `message`, `context` (object) |
| `cache`   | `event` (`hit\|missed\|written\|forgotten`), `key`, `store`, `value_preview?`, `ttl?` |
| `http`    | `method`, `url`, `status` (int), `duration_ms` (float) — an outgoing HTTP client request (curl / Guzzle / PSR-18) |

Keep payloads small; truncate large values (e.g. dump HTML, bindings) to a sane cap
(say 256 KiB per frame — Yerd drops over-long lines). The Yerd side maps each frame to
a `DumpEvent` and filters by `category` for the tabs (All/Dumps/Queries/Jobs/Views/
Requests/Logs/Cache/HTTP). Outgoing-`http` capture is opt-in (off by default).

### 2.3 Configuration & on/off — `state.json` + one INI directive
The extension is told **where** its state file is via a single INI directive that
**the extension registers itself** in MINIT:

- INI name: **`yerd_dump.state_path`** (type **`PHP_INI_SYSTEM`**), registered via
  `zend_register_ini_entries` (ext-php-rs INI API) in `MINIT`. Yerd passes the value
  with `php-fpm -d yerd_dump.state_path=/abs/path/state.json`.
  **Critical:** an *unregistered* `-d ini.name` is invisible to `ini_get()` — it only
  works because the extension registers it first. (Fallback: `get_cfg_var()`.)
- The extension reads no environment variables (FPM runs with a scrubbed near-empty
  env). All config comes from the INI directive + the state file.

**`state.json`** (written atomically by Yerd, read by the extension at RINIT):

```jsonc
{
  "enabled": true,
  "port": 2304,
  "features": {
    "dumps": true, "queries": true, "jobs": true, "views": true,
    "requests": true, "logs": true, "cache": true, "http": false
  }
}
```

At RINIT the extension reads this (cheap; OS page-caches it). If the file is missing,
unreadable, or `enabled=false`, the extension **does nothing** for the request (fast
path: one stat+read, then return). Per-feature flags gate individual observers.
Toggling never requires an FPM restart — only the file changes.

## 3. Engine integration

### 3.1 Lifecycle
- **MINIT**: register the `yerd_dump.state_path` INI entry; register `zend_observer`
  fcall observers (see below). Observer registration is cached by PHP per function
  definition, so non-target calls cost nothing.
- **RINIT**: read `state.json`; if disabled, set a per-request "off" flag and skip all
  work. If enabled, generate `request_id` and (lazily) prepare the socket.
- **RSHUTDOWN**: emit the `request` summary frame (method/uri/status/duration from
  superglobals); close the socket.
- **MSHUTDOWN**: tidy up.

### 3.2 Observed symbols (`zend_observer` via ext-php-rs `FcallObserver`)
`begin(&ExecuteData)` / `end(&ExecuteData, Option<&Zval>)` give first-class access to
`$this`, the call arguments (pre-loaded arg parser), and the return `Zval` — so reading
SQL/args/return is straightforward (the residual work is cheap, panic-safe
`Zval`→string rendering). Use `should_observe` to filter by class+method.

- **dumps** — observe **one** chosen symbol (decide in Phase 0, pin it in this doc):
  the global `dump`/`dd` functions, `Symfony\Component\VarDumper\VarDumper::dump`, or
  `DataDumperInterface::dump`. Render the dumped value to HTML+text and emit. `dd()`
  ends in `exit`; the observer's `begin/end` fires before the `exit` unwinds, so a
  synchronous emit there is captured. **Preserve normal output** — don't swallow the
  user-visible dump.
- **queries** — observe `PDOStatement::execute` / `PDO::exec` / `PDO::query`. This is
  **framework-agnostic** (works for any PHP app). Capture SQL, bound params, duration
  (time the call in begin/end), and caller `file:line`. Optionally enrich from
  Laravel's `QueryExecuted` event when present (connection name).
- **jobs / views / cache / logs** — observe `Illuminate\Events\Dispatcher::dispatch`
  (and the logger) and filter the event object's class:
  `JobProcessing`/`JobProcessed`/`JobFailed`, view `composing:`/`creating:` events,
  `CacheHit`/`CacheMissed`/`KeyWritten`/`KeyForgotten`, and log events. These are the
  Laravel-specific signals; everything funnels through the dispatcher, so one
  observation point covers most.
- **request** — no observation needed; assemble from superglobals at RINIT/RSHUTDOWN.

### 3.3 Caller resolution (`file:line`)
The screenshot shows the originating app/vendor frame (e.g.
`app/Actions/Plugins/PluginCache.php:36`). Walk the call stack from the observed frame
outward and pick the first frame outside the framework's query/dump internals.
`ExecuteData` gives the executing frame; use `prev_execute_data` to climb.

## 4. Robustness (non-negotiable — native code runs in every request)

A panic or segfault in an observer **takes down the FPM worker**. Rules:
- **Never panic across the FFI boundary.** Wrap observer bodies so any Rust panic is
  caught and swallowed (telemetry must never break the app).
- **Non-blocking socket.** Connect with a tiny timeout (~50 ms); set non-blocking;
  fire-and-forget writes. Attempt the connect **at most once per request**; if the
  server is down, set a per-request flag and silently no-op the rest of the request.
- **Cheap when disabled.** The disabled fast-path is one `state.json` read then return.
- **Bounded work.** Truncate large renders/bindings before sending; never allocate
  unboundedly from user data.

## 5. Build, ABI, and distribution

### 5.1 ABI matching (why per-PHP-minor artifacts)
A PHP extension's `ZEND_EXTENSION_BUILD_ID` encodes the module API version, ZTS,
and debug flags. PHP refuses to load a `.so` whose build-id doesn't match the engine.
`ZEND_MODULE_API_NO` is **stable within a released minor** (all 8.3.x share it), so
**one artifact per PHP minor** is correct. The other dimensions (NTS, glibc/macOS,
arch) are fixed by how Yerd ships PHP, so they're statically known — no runtime
introspection needed.

**Build against the same PHP that Yerd ships** (static-php.dev: `gnu-bulk`/glibc on
Linux, the macOS channel on macOS, all **NTS**) so the build-id matches. Mismatched
NTS/debug/minor → load failure or crash.

### 5.2 Build matrix
Per **(PHP minor × OS × arch)**, all **NTS**, glibc on Linux:

| PHP minor | macOS arm64 | macOS x86_64 | linux x86_64 (glibc) | linux aarch64 (glibc) |
|-----------|:-:|:-:|:-:|:-:|
| 8.3 / 8.4 / 8.5 / … | ✓ | ✓ | ✓ | ✓ |

Build with `ext-php-rs` (Rust); use the matching `php-config`/headers per target. CI
builds each cell and publishes a `.so` to GitHub Releases.

### 5.3 The download contract (`manifest.json` + per-asset SHA-256)
Yerd does **not** guess asset filenames. Each release publishes a
**`manifest.json`** describing every built `.so`, and Yerd resolves the right one
from it. The manifest is an object with a `files` array; each entry is:

```jsonc
{
  "name":   "…",       // the release-asset filename to download
  "php":    "8.3",     // PHP minor (major.minor)
  "os":     "macos",   // host OS as std::env::consts::OS spells it: macos | linux
  "arch":   "aarch64", // host arch as std::env::consts::ARCH spells it: aarch64 | x86_64
  "sha256": "…"        // hex SHA-256 of the asset, lowercase
}
```

Yerd fetches `manifest.json` and each asset from the **`latest`** release
(`https://github.com/forjedio/yerd-php-ext/releases/latest/download/<name>`), so
new releases are picked up automatically. For each installed PHP minor it finds the
entry matching `(php, os, arch)`, downloads `name`, **verifies the SHA-256 against
`sha256`** (mismatch → rejected), and places it atomically at
`{yerd-data}/php-ext/php-<minor>/yerd-dump.so`, then wires `-d extension=` to it. If
the on-disk `.so` already hashes to the manifest value, the download is skipped.
A minor with no matching manifest entry is left without capture.

The asset filenames inside the manifest are the extension repo's own concern -
Yerd only ever reads `name` from the manifest, so the naming scheme can change
without a Yerd release. **Use the `.so` suffix on all targets** (macOS `dlopen`s
`.so` fine) so loading stays uniform.

## 6. Out of scope
- Windows (Yerd's PHP is macOS/Linux today).
- ZTS builds (FPM is non-threaded → NTS only).
- End-user installation flows (Yerd owns download + loading).
- The dump server, ring buffer, GUI, config — those are **Yerd-side** (other repo).
