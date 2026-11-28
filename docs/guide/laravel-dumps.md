# Insights

**Insights** shows how a site's requests perform and where its database work goes
wrong. Pick a site, start recording, and Yerd times every PHP request it serves,
groups them by route, and flags query problems such as N+1s, duplicate queries,
slow queries and oversized reads. Alongside the numbers you get the **Live Feed**:
`dump()` / `dd()` output, queries, jobs, views, logs, cache events and outgoing
HTTP calls, streamed as your app runs.

It all lives on the **Insights** page under the **Developer** group in the
[desktop app](./desktop-app)'s side navigation. **Your application needs no
changes**: no `composer require`, no service provider, no `auto_prepend_file`.

::: info Recording is per site
Insights records nothing until you start recording a site, and starting one site
leaves every other site untouched. Sites that aren't recording cost next to
nothing: no timing is kept for them and the PHP extension does no work for their
requests.
:::

::: tip Works best with Laravel
Timing, routes and status codes work for any PHP site Yerd serves through
PHP-FPM. Queries and dumps are framework-agnostic (queries are observed at the
`PDO` level). Route templates such as `/orders/{order}`, and the jobs, views, logs
and cache categories, come from Laravel; other apps are grouped by their
normalised URL path, or by script for sites that run `.php` files directly.
:::

## Start recording

1. Open **Insights** and choose a site from the **site picker** at the left of
   the toolbar. Sites that are recording show a green dot in the picker.
2. Click **Start recording** (or the pill next to the picker). The pill turns
   green and reads **Recording · this site**; click it again to stop.
3. Load any page on the site. Static files (CSS, JS, images) are skipped and
   counted separately; the first PHP request appears within a couple of seconds.

<ThemedImage light="/images/insights-empty-light.png" dark="/images/insights-empty-dark.png" alt="The Insights page for acme-shop.test, which is not recording: the toolbar shows Not recording and the page offers a Start recording button" />

The rest of the toolbar:

- **5m / 1h / 24h** picks the time range every tab shows.
- **Pause** freezes the view so you can read it without it updating. It only
  affects what you see: recording carries on.
- The **trash** button clears everything recorded for the site (requests, chart
  points and dismissed issues). Recording stays on or off as it was.
- The **sliders** button opens [Insights settings](#thresholds-and-capture-settings).

The page has four tabs: **Overview**, **Requests**, **Issues** and **Live Feed**.

## Overview

The Overview summarises the selected range:

- **KPI tiles:** **P50 · warm** and **P95 · warm** (median and 95th-percentile
  response time of warm requests), **Requests** (with a per-minute rate and the
  number of static files skipped), **Failed** (the share of requests that returned a
  5xx or never reached PHP, with the 5xx and 4xx counts), and **Queries / request** (the average, plus the most
  queries a single request ran and on which route). On 5m and 1h, the P50 tile
  compares with the previous window of the same length, and the P95 tile points
  out a spike when one point in the range was much slower than the rest.
- **Status line:** how many [cold requests](#cold-starts) were set aside, and
  whether anything was dropped ("Every request recorded, 0 samples dropped", or
  how many requests and extension frames were lost).
- **Latency over time:** the p50 line, the p95 band and error dots across the
  range.
- **Where the time goes:** an average warm request split into **PHP /
  application**, **Database**, **Outgoing HTTP** and **Waiting for a worker**.
- **Detected issues:** the top issues in the range (see [Issues](#issues)).
- **Routes / Requests / Queries:** routes sorted by total time spent (requests,
  p50, p95, time share, queries per request and issue types), the slowest 10
  requests in the range, and the most expensive query shapes with their run
  count, total and mean time.

<ThemedImage light="/images/insights-overview-light.png" dark="/images/insights-overview-dark.png" alt="The Insights Overview for acme.test over the last hour while recording: KPI tiles for P50, P95, requests, failed and queries per request, the cold-request status line, the latency chart with a p95 spike, where the time goes, and the detected issues list" />

The time is the proxy's: what the browser actually waited for. Work your app does
after the response has been sent (for example Laravel's terminate middleware) is
not counted.

## Requests

The **Requests** tab lists recorded requests with their method, path, status,
time, query and dump counts. Filter by path, status or route, and switch between
**Slow first** and **Recent**.

Pick a request to see where its time went: a header with the route and status, a
breakdown bar, a callout for each issue it triggered with **Open in editor** for
the code that caused it, and the **event timeline** of its queries, dumps, cache,
log, HTTP, job and view events. Chips filter the timeline by type, and the
repeated queries of an N+1 are grouped into one row you can expand to see each
binding. In a narrow window the list becomes a table and the request opens in a
drawer.

<ThemedImage light="/images/insights-requests-light.png" dark="/images/insights-requests-dark.png" alt="The Requests tab sorted Slow first, with a cold-start request at the top and GET /orders/1042 selected: its time breakdown bar, an N+1 callout with Open in editor, and the event timeline with the 48 repeated queries grouped into one row" />

## Issues

Insights checks the queries of every request against five rules:

| Issue | Flagged when |
|---|---|
| **N+1** | The same query shape runs from the same line of code at least 5 times in one request, with different values (bound or inline). |
| **Duplicate** | The exact same SQL with the same bindings runs at least twice in one request. |
| **Slow query** | A single query takes longer than 100 ms. |
| **Large read** | A read without `LIMIT` returns at least 1,000 rows. |
| **Too many queries** | A request runs more than 50 queries. |

The numbers are the defaults; change them in
[the settings](#thresholds-and-capture-settings). The **Issues** tab lists every
issue in the range with how often it happened, how many requests it affected and
the time it cost, and chips narrow the list to one type. **Dismiss** an issue you
don't intend to fix; it moves under the **Dismissed** chip and stops counting
toward the badge next to **Insights** in the sidebar, which shows the open
issues across your recording sites.

<ThemedImage light="/images/insights-issues-light.png" dark="/images/insights-issues-dark.png" alt="The Issues tab listing five open issues for acme.test: two N+1s, a duplicate query, a slow query and a large read, each with its route and time cost, plus type chips and a Dismissed chip" />

## Live Feed

The **Live Feed** tab is the raw event stream for the selected site, grouped by
request: dumps, queries, jobs, views, request summaries, log writes, cache events
and outgoing HTTP calls. A request's detail has category chips with per-request
counts, an event filter, and a caller badge on each event: hover it for the
project-relative path, click it to open the file in your editor.

By default the feed keeps the **25 most recent requests**. Turn on **Keep
history** to keep every request instead, up to a buffer of 2,000 events. The
trash button above the list clears it.

**Pop out** opens the feed in its own **Live feed** window, so you can keep it
beside your editor and browser. It follows the site selected in Insights, shows
it in its title bar, and has a **pin on top** button. The tray's **Dumps** item
and `⇧⌘D` (`Ctrl+Shift+D`) open the same window.

<ThemedImage light="/images/insights-live-feed-light.png" dark="/images/insights-live-feed-dark.png" alt="The pop-out Live feed window for acme with the dump server live on port 2304: recent requests on the left and GET /orders/1042 selected, showing its cache hit, queries, an Order model dump, a queued job and a Blade view" />

The Live feed only shows sites that are recording, since those are the only
requests the extension reports.

| Category | What it captures |
|---|---|
| **Dumps** | `dump()`, `dd()`, `ddd()` calls (rendered HTML plus a plain-text fallback) |
| **Queries** | Eloquent / PDO database queries, with SQL, bindings, and timing |
| **Jobs** | Dispatched queue jobs and their outcome (processing / processed / failed) |
| **Views** | Rendered Blade views and their bound data keys |
| **Requests** | A summary of each incoming HTTP request (method, URI, status, duration) |
| **Logs** | Log writes, with level and context |
| **Cache** | Cache hits, misses, writes, and forgets |
| **HTTP** | Outgoing HTTP client calls (curl / Guzzle / PSR-18) |

A log entry's context array is printed as indented JSON under its message, and
entries with no context show just the level and message. Every context value is
shown, including ones that aren't strings: numbers, booleans and `null` appear as
text (`"1"`, `"true"`, `"null"`), and arrays and objects as their dump-style
rendering.

## Cold starts

PHP-FPM starts workers on demand and stops them when idle, so the first request
after a quiet spell pays for a fresh worker. Insights spots these **cold
starts** (the first request a new worker serves), keeps them out of the latency
figures so they don't distort p50 and p95, and counts them in the status line.
They still appear in the request list, marked **cold start, not in stats**.

Cold-start detection needs the [updated PHP extension](#the-php-extension).

## Thresholds and capture settings

The **sliders** button in the toolbar opens **Insights settings**:

- **Issue thresholds:** **N+1 queries** (at least 5 times), **Duplicate queries**
  (at least 2 times; one query is never a duplicate), **Slow query** (slower
  than 100 ms), **Large read** (at least 1,000 rows) and **Too many queries**
  (more than 50). Every value must be at least 1. Changes apply to requests
  recorded from then on.
- **Capture from the PHP extension:** the global switch for extension data on
  every recording site. Off keeps timing only: requests are still timed and
  grouped, but no queries, dumps or other events are collected and the Live feed
  stays empty.
- **Dump server:** the loopback address the extension reports to
  (`127.0.0.1:2304` by default) and whether it's running. **Change port** opens
  **Settings ▸ Application Ports**.
- **Captured signals:** one switch per Live feed category. **Outgoing HTTP** is
  off by default because it adds a little overhead; the others are on. Changes
  take effect on the next request, with no PHP restart.
- **PHP extension:** each installed PHP version with **INSTALLED**, **NOT
  INSTALLED**, or **TOO OLD: TIMING ONLY** for legacy versions.

<ThemedImage light="/images/insights-settings-light.png" dark="/images/insights-settings-dark.png" alt="The Insights settings dialog: the five issue thresholds, the Capture from the PHP extension switch, the dump server on 127.0.0.1:2304 with Change port, the captured signal switches with Outgoing HTTP off, and the extension status for PHP 7.4, 8.3, 8.4 and 8.5" />

## The PHP extension

Timing comes from Yerd's proxy and works with any PHP version. Everything else
(queries, dumps, cold starts, Laravel route names) comes from a native PHP
extension, `yerd-dump`, that observes your app at the engine level.

Yerd downloads it automatically while capture is on (the default): on startup it fetches the matching build for
**each installed PHP version** from the
[`forjedio/yerd-php-ext`](https://github.com/forjedio/yerd-php-ext) releases,
verifies it by SHA-256, and stores it alongside your PHP installs at
`{data}/php-ext/php-<version>/yerd-dump.so`. When a newer build replaces the one
on disk, Yerd restarts just that version's running PHP-FPM pool so it loads. The
same check runs each time you start recording a site, so a newer build is
picked up without restarting Yerd.

::: info Timing only without the updated extension
Insights needs a release of the extension that speaks Yerd's current protocol. An
older build is loaded but stays inert, so it never captures every site at once.
Until the update is in place, recording sites show timing only, with the hint
**"Timing only: install or update the Yerd PHP extension to see queries, dumps
and cold starts."**
:::

::: info Needs a matching released build
The extension is ABI-specific: there is one build per PHP minor, per OS, per
architecture. If a build for your exact PHP version and platform hasn't been
published yet, Yerd quietly skips that version, and its sites get timing only
until a matching build ships. The settings dialog shows, per installed PHP
version, whether the extension is present.

The download is **best-effort**: a network or verification failure is logged and
leaves your sites running normally, just without extension data.
:::

::: warning Legacy PHP is timing only
[Legacy PHP versions](./php-versions#legacy-php-versions) (7.4 / 8.0 / 8.1) have
no `yerd-dump` build. Sites on them still get request timing, routes by path and
status codes, but no queries, dumps or cold-start detection. The settings dialog
marks them **TOO OLD: TIMING ONLY**.
:::

## Storage

Insights keeps a rolling **24 hours** per site:

- The last few minutes live in memory; older data is written to disk in batches
  (every 5 minutes, or sooner on a busy site), so recording never waits on the
  disk.
- Data is kept under Yerd's state directory in an `insights/` folder, one folder
  per site, and **survives daemon restarts and reboots**.
- Data older than 24 hours is deleted automatically, and all sites together are
  capped at **2 GiB**. If a very busy site pushes past the cap, the oldest hours
  across all sites go first.
- Stopping a site's recording writes out what it has; the data stays viewable
  until it ages out. Use the trash button to delete it straight away.

A daemon crash can lose the last unwritten batch (up to about 5 minutes).

## How it works

**The proxy is the clock, the extension is the narrator.** For a recording site,
Yerd's built-in proxy times every request it hands to PHP-FPM, from arrival to
the last byte of the response, and gives the request a unique trace id. It passes
that id to PHP with the request, and the `yerd-dump` extension tags everything it
observes during the request with it, then sends a short summary at the end
(including which worker served it and the Laravel route that matched). Yerd's
[daemon](./daemon) joins the two by id, works out the route, the time breakdown
and any issues, and adds the request to per-minute summaries for the charts.

For sites that aren't recording, the proxy sends no trace id and the extension
does nothing for the request: no file read, no socket, no events.

The extension sends its events over a small TCP server the daemon runs on
**loopback only** (`127.0.0.1`, default port **2304**). Everything stays on your
machine: nothing is sent anywhere else, and if the port is already in use the
daemon logs it and retries rather than failing. Recording is never allowed to
take your sites down or slow them noticeably.

::: info No CLI commands
Insights is driven from the desktop app; there are no `yerd` CLI subcommands for
it. Its settings are stored in your [config file](../reference/configuration):
the recording sites and thresholds under `[insights]`, and the capture switch,
port and captured signals under `[dumps]`. The app keeps both in sync.
:::

## See also

- [PHP Versions](./php-versions) - the FPM pools the extension loads into.
- [The Desktop App](./desktop-app) - where the Insights page and the Live feed window live.
- [Configuration Reference](../reference/configuration) - the `[insights]` and `[dumps]` tables on disk.
