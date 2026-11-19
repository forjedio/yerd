/**
 * Pure derivations for the Dumps screen and the Dumps window: merging the
 * incremental `list_dumps` pages, grouping events into requests, and the text
 * and tags each event card shows. No I/O.
 */
import type { DumpCategory, DumpEvent, DumpsResponse, DumpsStatusResponse } from "@/ipc/types";
import { logRowBody } from "@/lib/dumpLog";

/** A payload value as display text; `null` and `undefined` read as empty. */
export function str(v: unknown): string {
  return v === undefined || v === null ? "" : String(v);
}

/** Top-level project directories that mark where the app root ends, so an
 *  absolute path like /Users/me/Herd/blog/app/Foo.php shows as app/Foo.php. */
export const PATH_ANCHORS = new Set([
  "app",
  "vendor",
  "routes",
  "config",
  "database",
  "bootstrap",
  "public",
  "resources",
  "storage",
  "tests",
  "src",
  "lib",
  "packages",
  "modules",
  "Modules",
  "nova-components",
]);

/** Strip everything before the first project-root anchor directory. */
export function projectRelative(file: string): string {
  const parts = file.split("/");
  const idx = parts.findIndex((p) => PATH_ANCHORS.has(p));
  return idx >= 0 ? parts.slice(idx).join("/") : file;
}

/** The last path segment. */
export function basename(p: string): string {
  const parts = p.split("/");
  return parts[parts.length - 1] || p;
}

/** Filename and line for the caller badge, e.g. `PluginCache.php:36`. */
export function rowCaller(e: DumpEvent): string {
  const file = str(e.payload.file);
  if (!file) return "";
  const line = str(e.payload.line);
  const name = basename(file);
  return line ? `${name}:${line}` : name;
}

/** Project-relative path and line for the caller tooltip, e.g. `app/Foo.php:36`. */
export function rowCallerRel(e: DumpEvent): string {
  const file = str(e.payload.file);
  if (!file) return "";
  const line = str(e.payload.line);
  const rel = projectRelative(file);
  return line ? `${rel}:${line}` : rel;
}

/** A duration in ms: two decimals in ms under a second, else in seconds. */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms)) return "";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms.toFixed(2)} ms`;
}

/** The event's own timing (`time_ms` for queries, `duration_ms` otherwise). */
export function rowDuration(e: DumpEvent): string {
  const raw = e.payload.time_ms ?? e.payload.duration_ms;
  return typeof raw === "number" ? formatDuration(raw) : "";
}

/** The returned/affected row count of a query, or null when unavailable. */
export function rowCount(e: DumpEvent): string | null {
  if (e.category !== "query") return null;
  const rc = e.payload.row_count;
  if (typeof rc !== "number") return null;
  return `${rc} ${rc === 1 ? "row" : "rows"}`;
}

/** The card body: the SQL, dump text, log line, job, view, URL or cache key. */
export function rowBody(e: DumpEvent): string {
  const p = e.payload;
  switch (e.category) {
    case "query":
      return str(p.sql);
    case "dump":
      return str(p.value_text) || str(p.value_html);
    case "log":
      return logRowBody(p);
    case "job":
      return [str(p.name), str(p.connection), str(p.status)].filter(Boolean).join(" · ");
    case "view":
      return str(p.name);
    case "request":
      return str(p.uri);
    case "http":
      return str(p.url);
    case "cache": {
      const key = str(p.key);
      const preview = str(p.value_preview);
      return preview ? `${key} = ${preview}` : key;
    }
    default:
      return JSON.stringify(p);
  }
}

/** The colour family of a tag, mapped to theme tokens by {@link TAG_TONE}. */
export type TagTone = "neutral" | "brand" | "success" | "warning" | "danger";

/** A tag's text and tone. */
export interface Tag {
  text: string;
  tone: TagTone;
}

/** Tailwind classes per tag tone, on the theme tokens. */
export const TAG_TONE: Record<TagTone, string> = {
  neutral: "bg-muted text-muted-foreground",
  brand: "bg-brand/15 text-brand",
  success: "bg-success/15 text-success",
  warning: "bg-warning/15 text-warning",
  danger: "bg-destructive/15 text-destructive",
};

/** The base class every tag shares. */
export const TAG = "inline-flex h-[18px] shrink-0 items-center rounded px-1.5 font-mono text-[11px] font-medium";

/** The tone of an HTTP status: 2xx success, 3xx warning, 4xx and 5xx danger. */
export function statusTone(code: number): TagTone {
  if (code >= 200 && code < 300) return "success";
  if (code >= 300 && code < 400) return "warning";
  if (code >= 400) return "danger";
  return "neutral";
}

function statusCode(raw: unknown): number | null {
  const code = typeof raw === "number" ? raw : Number.parseInt(str(raw), 10);
  return Number.isFinite(code) && code > 0 ? code : null;
}

/** The HTTP method tag of a request or outgoing HTTP event. */
export function methodTag(e: DumpEvent): Tag | null {
  if (e.category !== "http" && e.category !== "request") return null;
  const text = str(e.payload.method).toUpperCase();
  return text ? { text, tone: "brand" } : null;
}

/** The status-code tag of a request or outgoing HTTP event. */
export function statusTag(e: DumpEvent): Tag | null {
  if (e.category !== "http" && e.category !== "request") return null;
  const code = statusCode(e.payload.status);
  return code === null ? null : { text: String(code), tone: statusTone(code) };
}

/** The hit / miss / write / forget tag of a cache event. */
export function cacheTag(e: DumpEvent): Tag | null {
  if (e.category !== "cache") return null;
  switch (str(e.payload.event).toLowerCase()) {
    case "hit":
      return { text: "HIT", tone: "success" };
    case "missed":
    case "miss":
      return { text: "MISS", tone: "danger" };
    case "written":
    case "write":
      return { text: "WRITE", tone: "brand" };
    case "forgotten":
    case "forget":
      return { text: "FORGET", tone: "warning" };
    default: {
      const ev = str(e.payload.event);
      return ev ? { text: ev.toUpperCase(), tone: "neutral" } : null;
    }
  }
}

/** The category chip text of an event card. */
export const CATEGORY_LABEL: Record<DumpCategory, string> = {
  dump: "DUMP",
  query: "QUERY",
  job: "JOB",
  view: "VIEW",
  request: "REQUEST",
  log: "LOG",
  cache: "CACHE",
  http: "HTTP",
};

/**
 * Reconcile one `list_dumps` page into the held events: drop ids below the
 * server's `min_live_id` (evicted or cleared) and the removed ids, then append
 * the page's events not already held, so a re-sent page never doubles a row.
 * A page that changes nothing returns `held` itself, so an idle poll doesn't
 * regroup and re-render every request.
 */
export function mergePage(held: DumpEvent[], page: DumpsResponse): DumpEvent[] {
  const removed = new Set(page.removed_ids);
  const kept = held.filter((e) => e.id >= page.min_live_id && !removed.has(e.id));
  const have = new Set(kept.map((e) => e.id));
  const fresh = page.events.filter((e) => !have.has(e.id));
  if (fresh.length) return [...kept, ...fresh];
  return kept.length === held.length ? held : kept;
}

/** The `request` event of a request, parsed. It arrives when the request ends. */
export interface RequestSummary {
  method: string;
  uri: string;
  status: number | null;
  durationMs: number | null;
  ip: string;
}

/** The captured events of one web request. */
export interface DumpRequest {
  key: string;
  site: string;
  startedMs: number;
  firstId: number;
  /** Every event of the request, oldest first, the summary included. */
  events: DumpEvent[];
  summary: RequestSummary | null;
}

function parseSummary(e: DumpEvent): RequestSummary {
  const p = e.payload;
  return {
    method: str(p.method).toUpperCase(),
    uri: str(p.uri),
    status: statusCode(p.status),
    durationMs: typeof p.duration_ms === "number" ? p.duration_ms : null,
    ip: str(p.ip),
  };
}

/**
 * Group events by `request_id`, oldest event first within a request, newest
 * request first. An event without a request id stands alone.
 */
export function groupRequests(events: readonly DumpEvent[]): DumpRequest[] {
  const groups = new Map<string, DumpEvent[]>();
  for (const e of [...events].sort((a, b) => a.id - b.id)) {
    const key = e.request_id || `event-${e.id}`;
    const list = groups.get(key);
    if (list) list.push(e);
    else groups.set(key, [e]);
  }
  const out: DumpRequest[] = [];
  for (const [key, list] of groups) {
    const first = list[0];
    if (!first) continue;
    const summaryEvent = [...list].reverse().find((e) => e.category === "request");
    out.push({
      key,
      site: list.find((e) => e.site)?.site ?? "",
      startedMs: first.ts_ms,
      firstId: first.id,
      events: list,
      summary: summaryEvent ? parseSummary(summaryEvent) : null,
    });
  }
  return out.sort((a, b) => b.firstId - a.firstId);
}

/** The events a request draws as cards: all but its summary. */
export function requestCards(req: DumpRequest): DumpEvent[] {
  return req.events.filter((e) => e.category !== "request");
}

/** A detail category chip: `all` or one card category. */
export type ChipKey = DumpCategory | "all";

/** The detail's category chips, in drawn order. */
export const CATEGORY_CHIPS: readonly { key: ChipKey; label: string }[] = [
  { key: "all", label: "All" },
  { key: "dump", label: "Dumps" },
  { key: "query", label: "Queries" },
  { key: "job", label: "Jobs" },
  { key: "view", label: "Views" },
  { key: "log", label: "Logs" },
  { key: "cache", label: "Cache" },
  { key: "http", label: "HTTP" },
];

/** The chip label of a key. */
export function chipLabel(key: ChipKey): string {
  return CATEGORY_CHIPS.find((c) => c.key === key)?.label ?? key;
}

/** Card counts per chip for one request; `all` counts every card. */
export function requestCategoryCounts(req: DumpRequest): Record<ChipKey, number> {
  const counts = Object.fromEntries(CATEGORY_CHIPS.map((c) => [c.key, 0])) as Record<ChipKey, number>;
  for (const e of requestCards(req)) {
    counts.all += 1;
    counts[e.category] = (counts[e.category] ?? 0) + 1;
  }
  return counts;
}

/** Whether an event matches a free-text filter (case-insensitive; empty matches). */
export function matchesQuery(e: DumpEvent, q: string): boolean {
  const needle = q.trim().toLowerCase();
  if (needle === "") return true;
  const hay = `${e.category} ${e.site} ${rowBody(e)} ${rowCaller(e)} ${JSON.stringify(e.payload)}`;
  return hay.toLowerCase().includes(needle);
}

/**
 * The requests with at least one card the detail would show for this filter:
 * a card of the chip's category (any category for `all`) that matches the
 * filter text. The request summary is not a card, so it never matches alone.
 */
export function filterRequests(requests: readonly DumpRequest[], q: string, category: ChipKey): DumpRequest[] {
  return requests.filter((r) =>
    requestCards(r).some(
      (e) => (category === "all" || e.category === category) && matchesQuery(e, q),
    ),
  );
}

/** The request's title: its URI, else its site. */
export function requestTitle(req: DumpRequest): string {
  return req.summary?.uri || req.site;
}

/** A list duration: whole ms under a second, else seconds, e.g. "186 ms", "1.24 s". */
export function listDuration(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${Math.round(ms)} ms`;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** Local wall-clock time, `HH:MM:SS`. */
export function formatClock(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** "1 event" / "n events". */
export function eventsLabel(n: number): string {
  return `${n} ${n === 1 ? "event" : "events"}`;
}

/** The detail's meta line; parts the request lacks are left out, and the site
 *  only shows when the title is the path rather than the site. */
export function metaLine(req: DumpRequest): string {
  const parts = [req.summary ? req.site : "", formatClock(req.startedMs)];
  if (req.summary?.durationMs != null) parts.push(formatDuration(req.summary.durationMs));
  parts.push(eventsLabel(req.events.length));
  if (req.summary?.ip) parts.push(`from ${req.summary.ip}`);
  return parts.filter(Boolean).join(" · ");
}

/** The toolbar pill's state: not bound, intercepting, paused, or not loaded yet. */
export type CaptureState = "stopped" | "live" | "paused" | "unknown";

/** A missing bind wins over interception, which only decides live or paused. */
export function captureState(status: DumpsStatusResponse | null): CaptureState {
  if (!status) return "unknown";
  if (!status.running) return "stopped";
  return status.enabled ? "live" : "paused";
}

/** The pill text; `compact` is the narrow toolbar's shorter form. */
export function pillLabel(state: CaptureState, port: number, compact: boolean): string {
  switch (state) {
    case "live":
      return compact ? `Live · ${port}` : `Intercepting on ${port}`;
    case "paused":
      return "Paused";
    case "stopped":
      return compact ? "Stopped" : "Dump server stopped";
    default:
      return "";
  }
}

/**
 * The request to select under the follow-newest rule, given the newest key
 * before this update. Nothing selected, or a selection that is gone, picks the
 * newest; while the newest was selected, a newer arrival takes over; an older
 * pick stays put.
 */
export function nextSelection(
  prevFirst: string | null,
  selected: string | null,
  requests: readonly DumpRequest[],
): string | null {
  const first = requests[0]?.key ?? null;
  if (first === null) return null;
  if (selected === null || !requests.some((r) => r.key === selected)) return first;
  if (selected === prevFirst) return first;
  return selected;
}

/** One switch of the Captured signals list. */
export interface DumpSignal {
  key: string;
  label: string;
}

/** The daemon's per-feature switches, with the draft's labels. */
export const DUMP_SIGNALS: readonly DumpSignal[] = [
  { key: "dumps", label: "Dumps (dump / dd)" },
  { key: "queries", label: "Eloquent queries" },
  { key: "jobs", label: "Dispatched jobs" },
  { key: "views", label: "Blade views" },
  { key: "requests", label: "Incoming requests" },
  { key: "logs", label: "Log writes" },
  { key: "cache", label: "Cache events" },
  { key: "http", label: "Outgoing HTTP (off by default)" },
];
