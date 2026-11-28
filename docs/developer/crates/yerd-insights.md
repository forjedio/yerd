# yerd-insights

`yerd-insights` is the logic behind [Insights](../../guide/laravel-dumps), Yerd's
per-site request-performance dashboard. For every site the user turns recording
on for, the proxy times each PHP-FPM request, and the `yerd-php-ext` extension
narrates it (queries, dumps, logs, cache, outgoing HTTP, the matched Laravel
route, cold-start facts), joined by a proxy-supplied request id. This crate turns
those two inputs into finalised requests, per-minute rollups, detected query
issues, and the wire types the GUI reads, and stores the last 24 hours on disk.

**Proxy as the clock, extension as the narrator.** The proxy's timing is the
source of truth for counts, latency and errors. The extension only adds detail,
so a site whose PHP version has no (or an old) extension still gets timing.

The crate is consumed by [`yerdd`](../binaries/yerdd#insights-wiring-insights-rs),
which owns the locks, the background tasks and the IPC handlers. The proxy side
(the request tap and the response-body wrapper that measures each request) lives
in [`yerd-proxy`](./yerd-proxy)'s `insights` module, which does not depend on this
crate; the daemon maps its samples across field for field.

::: info Crate metadata
`description`: *Insights: per-site request timing, query issue detection, rollups
and on-disk codec for Yerd.* `#![forbid(unsafe_code)]`. Depends on
[`yerd-ipc`](./yerd-ipc) (the `Insights*` wire types and `DumpCategory`), `serde`
/ `serde_json`, `thiserror`, and `flate2` (one gzip member per stored request
detail). **No async runtime**: `tests/no_runtime_deps.rs` keeps `tokio`,
`anyhow`, `reqwest` and `openssl*` out of the default dependency graph.
:::

See also the [Crates overview](../crates), [`yerd-ipc`](./yerd-ipc) (the
request / response types), and the [IPC Protocol](../ipc-protocol) page.

## Module map

The crate follows the same `pure` / `io` split as [`yerd-mail`](./yerd-mail).

```text
src/
├── lib.rs            # re-exports + the purity-boundary doc
├── error.rs          # InsightsError (Io / Decode / Encode / SiteNotEmpty)
├── pure/
│   ├── aggregator.rs # Aggregator - joins samples and frames, finalises, flush batches
│   ├── contract.rs   # ProxySample, ExtFrame, ExtSummary, param names, trace_id()
│   ├── hist.rs       # LatencyHist - 64 geometric buckets, sparse
│   ├── route.rs      # route_key() / normalise_path() - how requests are grouped
│   ├── sql.rs        # fingerprint(), fnv(), is_read(), has_limit()
│   ├── issues.rs     # Thresholds, QueryEvent, detect() - N+1, duplicate, slow, ...
│   ├── model.rs      # FinalRequest / RequestDetail + their wire conversions
│   ├── rollup.rs     # MinuteRollup, overview(), open_issue_keys()
│   ├── dict.rs       # Dict - per-site route labels, SQL text, issue descriptors
│   ├── retention.rs  # hour keys, file names, site dir names, prune_plan()
│   ├── codec.rs      # .roll / .req NDJSON lines, .det gzip members
│   └── num.rs        # audited lossy numeric conversions
└── io/
    └── store.rs      # Store - the std-fs hour-file store
```

| Module | Role |
|--------|------|
| `aggregator` | The state machine. `on_sample` / `on_frame` / `tick` take `now` from the caller. Holds per-site rollups, the dictionary, the in-memory `recent` requests and the cross-site `pending` joins; hands out `FlushBatch`es and answers the overview, list and detail reads. |
| `contract` | The proxy and extension inputs and constants: `PROTOCOL_INI` (`yerd_dump.protocol`), `PROTOCOL_VERSION` (`2`), the four `YERD_TRACE_*` param names, `FEATURE_KEYS`, `trace_id(nonce, n)` and `features_param`. `ExtSummary::from_payload` parses the C4 request summary with every field defaulted. |
| `hist` | `LatencyHist`: bucket 0 is under 500 µs, bucket `i` tops out near `500 µs × 1.25^i`, bucket 63 (about 635 s) takes everything above. Bounds are integer steps of `× 5 / 4`, so they are identical on every platform. Quantiles report the bucket's geometric midpoint (about ±12.5%). |
| `route` | Grouping key precedence: the extension's Laravel route template, then a directly executed script other than `index.php` (non-front-controller sites), then the normalised path (query dropped, digits to `{id}`, UUIDs to `{uuid}`, 16+ hex chars to `{hash}`, at most 12 segments and 200 bytes). `route_id` is an FNV hash of the method and label. |
| `sql` | Single-pass fingerprinting: literals become `?`, `IN (...)` lists collapse to `IN (?+)`, whitespace collapses and bare words are lowercased; quoted identifiers are kept. A hand-written FNV-1a gives stable `u64` keys. |
| `issues` | `detect` over a request's `QueryEvent`s with `Thresholds` (defaults: N+1 at 5, duplicate at 2, slow at 100 ms, large read at 1000 rows, too many above 50). N+1 groups by `(fingerprint, file, line)` with at least two distinct concrete values, so bound parameters and inline literals both count; duplicates group by identical SQL plus bindings and skip members already reported as N+1. |
| `model` | `FinalRequest` (the list row, about 220 bytes) and `RequestDetail` (the drill-in timeline), plus `to_summary` / `to_ipc`. Issue keys cross the wire as 16-char hex (`key_hex` / `parse_key_hex`). |
| `rollup` | `MinuteRollup` (fixed-size numbers and `u64` keys only), `fold_request`, additive `merge`, and `overview()` for the 5m / 1h / 24h ranges. |
| `dict` | Per-site `Dict` of `DictEntry::{Route, Sql, Issue}` keyed by `u64`, with per-hour written-key tracking for the current and previous UTC hour only. |
| `retention` | `hour_of`, `file_name` / `parse_file_name` (`<YYYYMMDDHH>.roll`, `.req` or `.det`, UTC from epoch arithmetic), `site_dir_name` (keeps `[a-z0-9._-]`, hex-encodes anything else), and `prune_plan`. |
| `codec` | Encoders and decoders for the three file kinds. Decoders return `Err` on torn or garbage input, and callers skip the line. |
| `io::store` | `Store`: `append_batch`, `load_all`, `prune`, `scan_requests`, `find_detail`, `clear_site`, `save_dismissed` / `load_dismissed`. |

## Purity boundary

Everything under `pure/` is synchronous, clock-free and does no I/O. Every method
that depends on time takes `now` (Unix epoch milliseconds) from the caller, so
the aggregator's scenario tests run on a hand-advanced clock. `io/store.rs` is
the only filesystem code; it is plain synchronous `std::fs`, and the daemon calls
it from `spawn_blocking`, so this crate never needs an async runtime. The locks
around the aggregator and the ordering rules between flushing, clearing and
loading are the daemon's job (see
[`yerdd`'s Insights wiring](../binaries/yerdd#insights-wiring-insights-rs)).

## The aggregator

`Aggregator` keeps, per site, up to 1440 minute rollups plus not-yet-flushed
rollup deltas, the dictionary, the `recent` requests, the dismissed issue keys
and a running open-issue set (the GUI's sidebar badge). Across sites it keeps the
`pending` joins, a `v2_seen` set of PHP versions that have sent a protocol-2
summary since boot, and a 4096-entry `recently_finalised` set.

- **Untraced samples** (no trace params were sent: capture off, dump server not
  bound, legacy PHP, or a proxy error where PHP never ran) are finalised at once
  as timing-only and never count dropped frames.
- **Traced samples** wait in `pending` for the extension's summary for up to
  `GRACE_MS` (10 s). While the sample's PHP version has sent no v2 summary
  since boot and no frame has arrived for the request, the wait is only
  `PROBE_GRACE_MS` (1.5 s), so an old or missing extension does not hold
  timing-only requests back by the full grace. A summary that announces more frames than have arrived
  settles for up to `SETTLE_MS` (1 s) so frames still in flight are counted.
- **Frames** without a sample are discarded as orphans after `ORPHAN_MS` (30 s)
  of silence. Each request keeps at most `MAX_EVENTS_PER_REQUEST` (1000) events,
  and all pending joins together at most `PENDING_BYTES_CAP` (64 MiB).
- **Finalisation** computes the route key, runs issue detection, and splits the
  proxy's duration into app, database, outgoing HTTP and wait time. The proxy's
  duration is always the base, so work after `fastcgi_finish_request()` is
  excluded. A request whose summary says `worker_requests == 1` is a **cold
  start**: it is listed and counted, but kept out of the latency histograms,
  together with aborted and proxy-error requests.
- **Detail caps:** SQL 8 KiB, dump HTML 64 KiB, other strings 8 KiB, and 1 MiB
  per request, beyond which the detail is marked `truncated`.

Thresholds apply at finalisation, so changing them affects new requests only.

### Approximate capped-map merges

Each minute rollup caps its maps: 50 routes, 30 fingerprints and 30 issues per
minute. Route overflow folds into the reserved `OTHER_ROUTE` id; fingerprint and
issue overflow is counted in `capped` and dropped. Merging rollups (for the 1h
and 24h views, and on reload) is exact while every map stays under its cap and
**approximate once a cap overflows**. This is a deliberate bound on memory and
disk, not a counted loss: it does not show up in the dropped counters.

### Loss counters

Losses surface as two counters on the overview, shown in the GUI as
"N requests, M frames dropped":

| Counter | Counts |
|---------|--------|
| `dropped_requests` | Proxy samples lost because the tap channel was full, and requests in a batch whose disk write failed (they stay listed without detail). |
| `dropped_frames` | Extension frames missing per the C6 arithmetic (only once that PHP version has sent a v2 summary since boot), orphan frames, and frames refused by the per-request event cap or the pending-bytes cap. |

A failed write marks the batch's requests flushed so they still trim from memory,
and clears the affected hours' written-key sets so the dictionary entries are
rewritten by the next successful write. The rollup deltas stay in memory but are
missing on disk after a restart. A batch spanning several hours that fails
partway over-reports: hours already written still count as lost.

## Storage

The store lives at `{state}/insights` (the daemon passes the root). Layout:

```text
{state}/insights/
└── <site_dir>/              # site_dir_name(site)
    ├── 2026112714.roll      # NDJSON: rollup deltas + dictionary lines
    ├── 2026112714.req       # NDJSON: one request summary per line, with a .det locator
    ├── 2026112714.det       # one gzip member per request detail
    ├── ...                  # one trio per UTC hour (YYYYMMDDHH)
    └── dismissed.json       # dismissed issue keys, written temp-then-rename
```

- **`.roll`** lines are either a rollup delta `{"m":minute,...}` or a dictionary
  entry `{"d":key,"e":entry}`. There is **no separate dictionary file**: an entry
  is written into an hour's `.roll` the first time its key appears in that hour,
  before that hour's delta lines, so pruning an hour ages its entries out and a
  load rebuilds the dictionary from the retained files.
- **`.req`** lines are `FinalRequest`s carrying `loc: {offset, len}` into the
  same hour's `.det`. The hour is always `hour_of(summary.ts_ms)`, for writing and
  for `find_detail`.
- **`.det`** holds one gzip member per request (`Compression::fast()`), about
  4× smaller than the raw detail. A drill-in reads one hour's `.req` to find the
  locator, then does one positioned read of `.det`; a locator that decodes to a
  different id returns not found.

**Appends are crash tolerant without fsync.** Before appending, `.roll` and
`.req` have any torn tail truncated back to the last newline. `.det` is never
truncated: only complete `.req` lines point into it, so a torn member is just
unreferenced bytes, pruned with its hour. A torn line is skipped on load.

**Loading.** `load_all` reads up to 25 hours of `.roll` per site (rollups merged
by minute, plus dictionary entries tagged by hour), the current and previous
hour's `.req` for requests newer than 5 minutes (loaded as already flushed), and
`dismissed.json`. A missing root is an empty result, and a site that cannot be
read comes back empty with its error attached.

**Request lists.** The 1h / 24h list scans `.req` files newest first (`recent`
stops at the limit, `slowest` keeps a bounded heap) and caches the result as one
entry per site, at most 500 summaries. The entry is reused while its key and the
site's clear epoch match and either the store generation (bumped by every append,
prune and clear) is unchanged or the entry is younger than 30 s
(`SCAN_REUSE_MS`). `clear_site` drops the entry and bumps the clear epoch, so a
cached scan never crosses a clear.

### Flush and retention

Finalised requests stay in the aggregator's `recent` list until they are **both
flushed and older than 5 minutes** (`RECENT_MS`), so the last ~5 minutes are
served exactly from memory and unflushed requests are never lost from the list.
A site's buffered data is taken as a flush batch when any of these holds:

- 5 minutes have passed since its last flush (`FLUSH_INTERVAL_MS`),
- its unflushed bytes reach 8 MiB (`FLUSH_BYTES`),
- the flush is forced: the site's recording is turned off, or the daemon shuts down.

Retention has exactly two bounds, both applied by `prune_plan`:

1. **Age:** hour files that ended more than 24 hours ago are deleted. The daemon
   runs this once per UTC hour boundary.
2. **One global 2 GiB cap** across every site (`GLOBAL_CAP_BYTES`): while the
   total exceeds it, the **oldest hours across all sites** are deleted first. The
   daemon checks it whenever 128 MiB (`PRUNE_CHECK_BYTES`) has been appended since
   the last prune, so the overshoot is at most about 128 MiB.

There are no per-site or per-hour budgets, so one very busy site can evict other
sites' older hours. `prune` returns the removed `(site, hour)` pairs so the
aggregator can forget those hours' written-key sets.

**Crash semantics.** A daemon crash or `kill -9` loses at most the unflushed
batch (5 minutes or 8 MiB per site) and in-flight requests; data already written
survives. Power loss can also lose data the OS had not written back, since there
is no fsync by design. A crash after `.roll` but before `.req` leaves requests
counted in the rollups but missing from the list.

## Error model

`InsightsError` (`#[non_exhaustive]`, `thiserror`):

| Variant | Meaning |
|---------|---------|
| `Io { path, source }` | A filesystem operation on the store failed. |
| `Decode(String)` | A stored line or detail member could not be decoded (torn or garbage). |
| `Encode(String)` | A value could not be encoded for storage. |
| `SiteNotEmpty { site }` | `Aggregator::load_site` was called for a site that already holds state. |

## The extension contract

The extension (`forjedio/yerd-php-ext`) is built separately, so the protocol
between it and Yerd is fixed here. The constants live in `pure::contract`; the
proxy sets the params, the daemon sets the INI define and handles the frames.
The sections below are the contract both sides implement.

### C1. Mode selection (INI, set by Yerd)

| Item | Value |
| --- | --- |
| Directive | `yerd_dump.protocol` (new). The extension registers it in MINIT as `PHP_INI_SYSTEM`, default `"1"`, and reads it once in MINIT like `yerd_dump.state_path`. |
| Yerd | New Yerd always passes `-d yerd_dump.protocol=2` alongside `-d yerd_dump.state_path=…`, via `DumpExtSettings.ini_defines`. Old Yerd never passes it. |
| Extension, protocol `1` or unset (legacy mode) | Behaviour **identical to today**: read `state.json` each RINIT and honour `enabled/port/features`. `YERD_TRACE_*` params are ignored. The extension MAY add the C3 top-level fields; old Yerd ignores unknown top-level fields. |
| Extension, protocol `≥ 2` (trace mode) | **Never read `state.json`.** At RINIT, look up the FastCGI param `YERD_TRACE_ID`. If it is absent or empty, do nothing for this request: no ctx, no socket, no frames (the R2 zero-cost path). The lookup mechanism is the extension's choice. If present, arm the request using C2. |
| Old extension + new Yerd | The old extension does not know `yerd_dump.protocol` (PHP ignores unknown `-d` keys) and keeps reading `state.json`. New Yerd always writes `"enabled": false` there (C5), so the old extension captures nothing anywhere. Insights degrades to timing-only. |

### C2. Per-request FastCGI params

The proxy sets these only for Insights-enabled sites, and only when all of these hold: global capture is on, the dump server is bound, and the site's PHP version is not legacy (< 8.2 never has an extension build).

| Param | Format | Meaning |
| --- | --- | --- |
| `YERD_TRACE_ID` | 32 lowercase hex chars: a 16-hex daemon boot nonce followed by a 16-hex counter | The request id. The extension MUST use it verbatim as `request_id` on every frame of this request. |
| `YERD_TRACE_PORT` | decimal `1..=65535` | The dump-server loopback port. If it is missing or invalid, the extension does nothing. |
| `YERD_TRACE_SITE` | Yerd site name (e.g. `blog`) | The extension MUST put it in every frame's `site` field in trace mode, replacing `HTTP_HOST`. |
| `YERD_TRACE_FEATURES` | comma-separated subset of `dumps,queries,jobs,views,requests,logs,cache,http`, possibly empty | The categories to emit. Unknown keys are ignored. The `request` summary (C4) is **always** emitted in trace mode, whatever `requests` says. |

The names are not `HTTP_*`, so a client cannot inject them through request headers. They are visible to PHP in `$_SERVER`, which is harmless.

### C3. Frame envelope additions (trace mode, additive top-level keys)

`{category, ts, site, request_id, payload, v, seq, t_us}`

| Key | Type | Meaning |
| --- | --- | --- |
| `v` | integer `2` | Protocol marker. Yerd routes only `v >= 2` frames to Insights. |
| `seq` | integer ≥ 1 | Per-request frame sequence number, assigned **when the frame is built**, whether or not the write succeeds. Strictly increasing. Frames may arrive out of `seq` order (deferred query frames). |
| `t_us` | integer ≥ 0 | Microseconds from RINIT to the start of the event. For a query this is the execute start. |

The categories are unchanged; no new category is introduced.

### C4. Request summary (`category: "request"`, the last frame of the request, trace mode)

The existing payload keys stay: `method, uri, status, duration_ms, ip`. Added payload keys:

| Key | Type | Meaning |
| --- | --- | --- |
| `start_us` | integer | Wall-clock Unix epoch microseconds at RINIT. |
| `duration_us` | integer | Microseconds from RINIT to when the summary is built. |
| `worker_pid` | integer | `getpid()` of the FPM worker. |
| `worker_requests` | integer ≥ 1 | How many requests this worker process has handled, **every request, traced or not**, counted at RINIT and including this one. `1` means the first request served by a freshly spawned worker, i.e. a cold start (R6). |
| `route` | object or `null` | From `Illuminate\Routing\Events\RouteMatched`, which flows through the already-observed `Dispatcher::dispatch`: `{"uri": "<template as Laravel stores it, e.g. orders/{order}>", "name": "<route name or null>", "action": "<Controller@method or Closure>"}`. `null` when no route matched or the app is not Laravel. |
| `frames` | integer | Equals this summary's own `seq`: the total number of frames built, including the summary. |
| `frames_unsent` | integer | Frames built but not written. Informational. |

**Delivery rule.** The summary MUST be attempted even if earlier frames were lost. If the request's connection is unusable, the extension makes **one** fresh connect attempt (same 50 ms timeout) for the summary alone. The never-block and never-panic rules still apply.

### C5. `state.json` (Yerd → old extensions)

The shape is unchanged (`{enabled, port, features}`, pinned by `state_file_byte_shape`). New Yerd always writes `"enabled": false`. The trace-mode extension never reads the file.

### C6. How Yerd handles frames

- **No `v`, or `v < 2`:** the frame goes to the live ring only, as today.
- **`v >= 2`:** the frame goes to the ring **and** the aggregator, keyed by `request_id`.
- **Drop arithmetic:**
  - When the summary arrives: dropped = `frames − received` (received includes the summary).
  - When it never arrives, and the PHP version's "v2 seen" flag is set: dropped = `max(1, highest_seq_seen + 1 − received)`.
- **Received count:** `received` counts every frame that arrives, including frames refused by the event or byte caps. Refused frames are counted once, under the caps, so the C6 arithmetic never counts them a second time.
- **Orphan frames:** frames for an id the aggregator never gets a proxy sample for, within 30 s of the last frame seen for that id, are discarded and counted once. The count is `max(received, summary.frames)` if a summary arrived, else `received`.
- **Late frames:** frames arriving for an id finalised in the last ~4096 requests are discarded without being counted again (they were already accounted for).
- **Durations:** the extension's `duration_ms` stays a display field. The proxy's duration is authoritative.

### Compatibility summary

| Extension | Yerd | Result |
|-----------|------|--------|
| Protocol 2 | New | Full Insights for enabled sites; other sites pay one FastCGI-param lookup. |
| Old (state file) | New | `state.json` always says `enabled: false`, so it captures nothing anywhere. Insights is timing-only. |
| Protocol 2 | Old | No protocol define, so the extension behaves exactly as before (legacy mode). |

## Tests / invariants

- Table tests per `pure` module: histogram bounds and quantiles, route
  normalisation (with a pinned `id()`), fingerprints and hash constants, issue
  rules at their exact thresholds, contract parsing, rollup merges, dictionary
  retention and per-hour written keys, retention file names across day and year
  boundaries, and codec round trips.
- `pure/aggregator/tests.rs`: scenario tests on a hand-advanced clock (sample
  before and after summary, grace and settle, orphans, late frames, caps, cold
  starts, flush triggers, failed flushes, clear, `load_site`).
- `io/store/tests.rs`: tempdir tests for torn tails, double `.det` appends, prune
  by age and by cap, locator reads, the scan cache and clear.
- `tests/no_runtime_deps.rs`: no async runtime or HTTP stack in the default
  graph.

## See also

- [Insights guide](../../guide/laravel-dumps) - the user-facing feature
- [yerdd: Insights wiring](../binaries/yerdd#insights-wiring-insights-rs) - the tasks, locks and IPC handlers
- [yerd-ipc](./yerd-ipc) and [IPC Protocol](../ipc-protocol) - the `Insights*` requests and wire types
- [yerd-php](./yerd-php#dump-extension-loading) - how the extension and its INI defines are loaded into FPM
- [Config schema history](../config-schema-history) - the v35 `[insights]` table
- [Crates Overview](../crates)
- Source: [`crates/yerd-insights`](https://github.com/forjedio/yerd/tree/main/crates/yerd-insights)
