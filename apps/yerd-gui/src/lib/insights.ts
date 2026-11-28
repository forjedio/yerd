/**
 * Pure derivations for the Insights screen: number formatting, the KPI tiles,
 * the latency chart geometry, the time breakdown bar, the issue filters and the
 * request timeline. The daemon owns the statistics; this only shapes them for
 * display. No I/O.
 */
import type {
  DumpCategory,
  DumpEvent,
  InsightsEvent,
  InsightsIssue,
  InsightsIssueKind,
  InsightsOverview,
  InsightsPoint,
  InsightsRange,
  InsightsRequestDetail,
  InsightsRequestIssue,
  InsightsRequestSummary,
  InsightsThresholds,
  InsightsTimePart,
} from "@/ipc/types";
import { basename, CATEGORY_LABEL, formatClock, projectRelative, rowBody, statusTone } from "@/lib/dumps";
import { type ChipTone, plural } from "@/lib/utils";

/** The range picker's options, in order. */
export const RANGE_OPTIONS: readonly { value: InsightsRange; label: string }[] = [
  { value: "5m", label: "5m" },
  { value: "1h", label: "1h" },
  { value: "24h", label: "24h" },
];

/** Whether `v` is a range the daemon accepts. */
export function isRange(v: unknown): v is InsightsRange {
  return v === "5m" || v === "1h" || v === "24h";
}

/** The previous window a range's delta compares against, e.g. "previous hour". */
export function rangeLabel(range: InsightsRange): string {
  switch (range) {
    case "5m":
      return "previous 5 min";
    case "1h":
      return "previous hour";
    default:
      return "previous 24 hours";
  }
}

/** A number with thousands separators. */
export function formatCount(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** A millisecond figure split into value and unit: `68 ms`, `1.24 s`, `0.4 ms`. */
export function splitMs(ms: number | null | undefined): { value: string; unit: string } {
  if (ms == null || !Number.isFinite(ms)) return { value: "-", unit: "" };
  if (ms >= 1000) return { value: (ms / 1000).toFixed(2), unit: "s" };
  if (ms < 10 && ms > 0 && !Number.isInteger(ms)) return { value: ms.toFixed(1), unit: "ms" };
  return { value: String(Math.round(ms)), unit: "ms" };
}

/** A millisecond figure as one string, e.g. `68 ms`. */
export function formatMs(ms: number | null | undefined): string {
  const { value, unit } = splitMs(ms);
  return unit ? `${value} ${unit}` : value;
}

/** A byte count, e.g. `48.2 KB`. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 1024) return `${Math.max(0, Math.round(n || 0))} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Wall-clock `HH:MM` of an epoch-ms instant. */
export function formatHourMinute(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

const REASONS: Record<number, string> = {
  200: "OK",
  201: "Created",
  204: "No Content",
  301: "Moved Permanently",
  302: "Found",
  304: "Not Modified",
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  405: "Method Not Allowed",
  419: "Page Expired",
  422: "Unprocessable Content",
  429: "Too Many Requests",
  500: "Internal Server Error",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

/** A status with its reason phrase when known, e.g. `200 OK`. */
export function statusText(status: number): string {
  const reason = REASONS[status];
  return reason ? `${status} ${reason}` : String(status);
}

/** The tone of a request's duration: amber past 250 ms, red past a second. */
export function durationTone(ms: number): ChipTone {
  if (ms >= 1000) return "danger";
  if (ms >= 250) return "warning";
  return "neutral";
}

/** The tone of an HTTP method tag. */
export function methodTone(method: string): ChipTone {
  switch (method.toUpperCase()) {
    case "GET":
    case "HEAD":
      return "success";
    case "DELETE":
      return "danger";
    case "PUT":
    case "PATCH":
      return "warning";
    default:
      return "brand";
  }
}

export { formatClock, statusTone };

/** One KPI tile. */
export interface KpiTile {
  key: string;
  label: string;
  value: string;
  unit: string;
  /** Tints the value (failed % above zero). */
  valueTone: ChipTone;
  sub: string;
  subTone: ChipTone;
  /** A leading arrow on the sub-line. */
  trend: "up" | "down" | null;
}

/**
 * The five KPI tiles: warm p50 with its change against the previous window,
 * warm p95 with the range's spike, requests with rate and static skips, failed
 * share with the 5xx/4xx split, and the average queries per request with the
 * worst request. `maxQueries` is the "too many queries" threshold, which tints
 * the worst-request line.
 */
export function kpiTiles(o: InsightsOverview, maxQueries = 50): KpiTile[] {
  const k = o.kpis;
  const p50 = splitMs(k.p50_ms);
  const p95 = splitMs(k.p95_ms);
  const delta = k.p50_delta_ms;
  const deltaShown = delta != null && Math.round(Math.abs(delta)) > 0;
  const spike =
    k.p95_spike_ms != null && k.p95_spike_at_ms != null && k.p95_ms != null && k.p95_spike_ms > k.p95_ms;
  const avg = k.queries_avg;
  return [
    {
      key: "p50",
      label: "P50 · warm",
      value: p50.value,
      unit: p50.unit,
      valueTone: "neutral",
      sub: deltaShown ? `${formatMs(Math.abs(delta))} vs ${rangeLabel(o.range)}` : "Median warm request",
      subTone: deltaShown ? (delta < 0 ? "success" : "danger") : "neutral",
      trend: deltaShown ? (delta < 0 ? "down" : "up") : null,
    },
    {
      key: "p95",
      label: "P95 · warm",
      value: p95.value,
      unit: p95.unit,
      valueTone: "neutral",
      sub: spike
        ? `${formatMs(k.p95_spike_ms)} · spike at ${formatHourMinute(k.p95_spike_at_ms ?? 0)}`
        : "Slowest 5% of warm requests",
      subTone: spike ? "warning" : "neutral",
      trend: spike ? "up" : null,
    },
    {
      key: "requests",
      label: "Requests",
      value: formatCount(k.requests),
      unit: "",
      valueTone: "neutral",
      sub: `${formatRate(k.per_minute)}/min · ${formatCount(k.static_skipped)} static skipped`,
      subTone: "neutral",
      trend: null,
    },
    {
      key: "failed",
      label: "Failed",
      value: k.requests ? k.failed_pct.toFixed(1) : "-",
      unit: k.requests ? "%" : "",
      valueTone: k.failed_pct > 0 ? "danger" : "neutral",
      sub: `${formatCount(k.s5xx)} × 5xx · ${formatCount(k.s4xx)} × 4xx`,
      subTone: "neutral",
      trend: null,
    },
    {
      key: "queries",
      label: "Queries / request",
      value: avg == null ? "-" : formatAvg(avg),
      unit: avg == null ? "" : "avg",
      valueTone: "neutral",
      sub:
        k.queries_max > 0
          ? `max ${formatCount(k.queries_max)}${k.queries_max_route ? ` on ${k.queries_max_route}` : ""}`
          : avg == null
            ? "Needs the Yerd PHP extension"
            : "No queries yet",
      subTone: k.queries_max > maxQueries ? "warning" : "neutral",
      trend: null,
    },
  ];
}

function formatRate(perMinute: number): string {
  if (!Number.isFinite(perMinute) || perMinute <= 0) return "0";
  return perMinute < 10 ? perMinute.toFixed(1).replace(/\.0$/, "") : formatCount(perMinute);
}

function formatAvg(avg: number): string {
  return avg < 10 ? avg.toFixed(1).replace(/\.0$/, "") : formatCount(avg);
}

/** The line under the KPI tiles: cold starts set aside and recording losses. */
export function statusLine(o: InsightsOverview): { cold: string | null; dropped: string; lossy: boolean } {
  const cold = o.cold > 0 ? `${plural(o.cold, "cold request", "cold requests")} set aside (first hit after an idle worker spawn)` : null;
  const lossy = o.dropped_requests > 0 || o.dropped_frames > 0;
  const dropped = lossy
    ? `${plural(o.dropped_requests, "request", "requests")}, ${plural(o.dropped_frames, "frame", "frames")} dropped`
    : "Every request recorded, 0 samples dropped";
  return { cold, dropped, lossy };
}

/** Whether to show the "timing only" hint: requests arrived, none carried
 *  extension data, and the site's PHP version can load the extension. */
export function timingOnly(o: InsightsOverview): boolean {
  return !o.ext_seen && o.ext_available && o.kpis.requests > 0;
}

/** A computed SVG for the latency chart. */
export interface ChartGeometry {
  width: number;
  height: number;
  /** The p50 line, possibly several `M` runs where warm data is missing. */
  p50: string;
  /** The filled area under the p95 line. */
  p95: string;
  errors: { x: number; y: number; count: number }[];
  /** The x of the p95 spike marker, if any. */
  spikeX: number | null;
  /** Dashed gridline ys. */
  grid: number[];
  ticks: { x: number; label: string }[];
  yMax: number;
}

function niceCeil(v: number): number {
  if (v <= 0) return 10;
  const pow = 10 ** Math.floor(Math.log10(v));
  for (const step of [1, 2, 2.5, 5, 10]) {
    if (v <= step * pow) return step * pow;
  }
  return 10 * pow;
}

/**
 * The latency chart's paths in a `width` x `height` box: the p50 line, the area
 * under p95, a dot near the top for every point with errors, the spike marker,
 * three gridlines and five time ticks. Points without warm data break the line
 * and the area rather than dropping to zero.
 */
export function chartGeometry(
  series: readonly InsightsPoint[],
  width: number,
  height: number,
  spikeAtMs?: number,
): ChartGeometry {
  const pts = [...series].sort((a, b) => a.t_ms - b.t_ms);
  const first = pts[0]?.t_ms ?? 0;
  const last = pts[pts.length - 1]?.t_ms ?? 0;
  const span = last - first;
  const top = Math.max(0, ...pts.map((p) => p.p95_ms ?? p.p50_ms ?? 0));
  const yMax = niceCeil(top * 1.15);
  const plotTop = height * 0.18;
  const x = (t: number): number => (span > 0 ? ((t - first) / span) * width : width / 2);
  const y = (v: number): number => height - (Math.min(v, yMax) / yMax) * (height - plotTop);
  const r = (n: number): number => Math.round(n * 10) / 10;

  const runs = (pick: (p: InsightsPoint) => number | undefined): InsightsPoint[][] => {
    const out: InsightsPoint[][] = [];
    let cur: InsightsPoint[] = [];
    for (const p of pts) {
      if (pick(p) == null) {
        if (cur.length) out.push(cur);
        cur = [];
      } else cur.push(p);
    }
    if (cur.length) out.push(cur);
    return out;
  };

  const p50 = runs((p) => p.p50_ms)
    .map((run) => run.map((p, i) => `${i ? "L" : "M"}${r(x(p.t_ms))},${r(y(p.p50_ms ?? 0))}`).join(""))
    .join("");
  const p95 = runs((p) => p.p95_ms)
    .map((run) => {
      const line = run.map((p, i) => `${i ? "L" : "M"}${r(x(p.t_ms))},${r(y(p.p95_ms ?? 0))}`).join("");
      const end = run[run.length - 1]?.t_ms ?? 0;
      const start = run[0]?.t_ms ?? 0;
      return `${line}L${r(x(end))},${height}L${r(x(start))},${height}Z`;
    })
    .join("");

  const errors = pts
    .filter((p) => p.errors > 0)
    .map((p) => ({ x: r(x(p.t_ms)), y: r(plotTop / 2), count: p.errors }));

  const spikeX =
    spikeAtMs != null && pts.length > 0 && spikeAtMs >= first && spikeAtMs <= last ? r(x(spikeAtMs)) : null;

  const grid = [0.25, 0.5, 0.75].map((f) => r(plotTop + (height - plotTop) * f));
  const ticks = pts.length
    ? [0, 0.25, 0.5, 0.75, 1].map((f) => ({ x: r(f * width), label: formatHourMinute(first + span * f) }))
    : [];

  return { width, height, p50, p95, errors, spikeX, grid, ticks, yMax };
}

/** Display names of the time breakdown parts, in bar order. */
export const PART_LABEL: Record<InsightsTimePart, string> = {
  app: "PHP / application",
  db: "Database",
  http: "Outgoing HTTP",
  wait: "Waiting for a worker",
};

/** The short names the drill-in's legend uses. */
export const PART_SHORT: Record<InsightsTimePart, string> = {
  app: "App",
  db: "Database",
  http: "HTTP",
  wait: "Queue wait",
};

/** The fill class of each part's bar segment and legend swatch. */
export const PART_FILL: Record<InsightsTimePart, string> = {
  app: "bg-brand",
  db: "bg-warning",
  http: "bg-violet-400",
  wait: "bg-faint",
};

const PART_ORDER: readonly InsightsTimePart[] = ["app", "db", "http", "wait"];

/** One segment of the breakdown bar. */
export interface BarSegment {
  part: InsightsTimePart;
  label: string;
  short: string;
  fill: string;
  ms: number;
  /** Fraction of the bar, 0..1. */
  share: number;
}

/**
 * The breakdown bar's segments in app, db, http, wait order, dropping parts with
 * no time. A part's `share` is used as given, else derived from its share of the
 * summed milliseconds.
 */
export function breakdownBar(parts: readonly { part: InsightsTimePart; ms: number; share?: number }[]): BarSegment[] {
  const total = parts.reduce((s, p) => s + Math.max(0, p.ms), 0);
  return PART_ORDER.flatMap((part) => {
    const p = parts.find((x) => x.part === part);
    if (!p) return [];
    const share = p.share ?? (total > 0 ? Math.max(0, p.ms) / total : 0);
    if (share <= 0 && p.ms <= 0) return [];
    return [{ part, label: PART_LABEL[part], short: PART_SHORT[part], fill: PART_FILL[part], ms: p.ms, share }];
  });
}

/** A request's own breakdown parts from its summary. */
export function requestParts(s: InsightsRequestSummary): { part: InsightsTimePart; ms: number }[] {
  const out: { part: InsightsTimePart; ms: number }[] = [];
  if (s.app_ms != null) out.push({ part: "app", ms: s.app_ms });
  if (s.db_ms != null) out.push({ part: "db", ms: s.db_ms });
  if (s.http_ms != null) out.push({ part: "http", ms: s.http_ms });
  if (s.wait_ms != null) out.push({ part: "wait", ms: s.wait_ms });
  return out;
}

/** The issue type's tag text. */
export const ISSUE_LABEL: Record<InsightsIssueKind, string> = {
  n_plus_one: "N+1",
  duplicate: "Duplicate",
  slow_query: "Slow query",
  large_read: "Large read",
  too_many_queries: "Too many",
};

/** The issue type's tag tone. */
export const ISSUE_TONE: Record<InsightsIssueKind, ChipTone> = {
  n_plus_one: "warning",
  duplicate: "brand",
  slow_query: "danger",
  large_read: "neutral",
  too_many_queries: "warning",
};

/** An issue filter chip key. */
export type IssueChipKey = "all" | InsightsIssueKind | "dismissed";

const ISSUE_CHIP_ORDER: readonly { key: IssueChipKey; label: string }[] = [
  { key: "all", label: "All" },
  { key: "n_plus_one", label: "N+1" },
  { key: "duplicate", label: "Duplicates" },
  { key: "slow_query", label: "Slow" },
  { key: "large_read", label: "Large read" },
  { key: "too_many_queries", label: "Too many" },
  { key: "dismissed", label: "Dismissed" },
];

/** The issues a chip keeps: open ones of every kind (`all`), open ones of one
 *  kind, or the dismissed ones. */
export function issueFilter(issues: readonly InsightsIssue[], key: IssueChipKey): InsightsIssue[] {
  if (key === "dismissed") return issues.filter((i) => i.dismissed);
  const open = issues.filter((i) => !i.dismissed);
  return key === "all" ? open : open.filter((i) => i.kind === key);
}

/** The issue chips with their counts: All and Dismissed always, a kind only
 *  when it has open issues or is the active chip. */
export function issueChips(
  issues: readonly InsightsIssue[],
  active: IssueChipKey,
): { key: IssueChipKey; label: string; count: number }[] {
  return ISSUE_CHIP_ORDER.map((c) => ({ ...c, count: issueFilter(issues, c.key).length })).filter(
    (c) => c.key === "all" || c.key === "dismissed" || c.count > 0 || c.key === active,
  );
}

/** A project-relative `file:line`, or "" without a file. */
export function callSite(file?: string, line?: number): string {
  if (!file) return "";
  const rel = projectRelative(file);
  return line ? `${rel}:${line}` : rel;
}

/** The issue row's headline: its SQL, or a count for "too many queries". */
export function issueTitle(i: InsightsIssue): string {
  if (i.kind === "too_many_queries") return `${formatCount(i.max_in_request)} queries in one request`;
  return i.sql ?? i.hint;
}

/** The issue row's detail line: what happened, where, and the hint. */
export function issueDetail(i: InsightsIssue): string {
  const what = ((): string => {
    switch (i.kind) {
      case "n_plus_one":
        return `${i.max_in_request}× in one request`;
      case "duplicate":
        return `Same SQL and bindings ${i.max_in_request}× in one request`;
      case "slow_query":
        return plural(i.occurrences, "slow run", "slow runs");
      case "large_read":
        return `${plural(i.occurrences, "read", "reads")} without a limit`;
      default:
        return plural(i.affected_requests, "request", "requests");
    }
  })();
  const parts = [what, callSite(i.file, i.line), i.kind === "too_many_queries" ? "" : i.hint];
  return parts.filter(Boolean).join(" · ");
}

/** The issue row's cost: per-request added time, or a slow query's mean. */
export function issueImpact(i: InsightsIssue): string {
  const per = i.affected_requests > 0 ? i.impact_ms / i.affected_requests : i.impact_ms;
  if (i.kind === "slow_query") {
    const mean = i.occurrences > 0 ? i.impact_ms / i.occurrences : i.impact_ms;
    return formatMs(mean);
  }
  return `+${formatMs(per)}`;
}

/** The drill-in callout's headline for one request issue. */
export function requestIssueTitle(i: InsightsRequestIssue): string {
  switch (i.kind) {
    case "n_plus_one":
      return `N+1: the same query ran ${i.count} times with different values`;
    case "duplicate":
      return `Duplicate: the identical query ran ${i.count} times`;
    case "slow_query":
      return `Slow query: ${formatMs(i.time_ms)}`;
    case "large_read":
      return "Large read: many rows fetched without a limit";
    default:
      return `Too many queries: ${i.count} in one request`;
  }
}

/** The drill-in callout's detail line. */
export function requestIssueDetail(i: InsightsRequestIssue): string {
  const where = i.file ? (i.line ? `${basename(i.file)}:${i.line}` : basename(i.file)) : "";
  return [where, `${formatMs(i.time_ms)} of this request`, i.hint].filter(Boolean).join(" · ");
}

/** The timeline chip keys. */
export type TimelineChipKey = "all" | DumpCategory;

const TIMELINE_CHIPS: readonly { key: DumpCategory; label: string }[] = [
  { key: "query", label: "Queries" },
  { key: "dump", label: "Dumps" },
  { key: "cache", label: "Cache" },
  { key: "log", label: "Logs" },
  { key: "http", label: "HTTP" },
  { key: "job", label: "Jobs" },
  { key: "view", label: "Views" },
];

/** The timeline filter chips: All, then each category the request has, with counts. */
export function timelineChips(events: readonly InsightsEvent[]): { key: TimelineChipKey; label: string; count: number }[] {
  const shown = events.filter((e) => e.category !== "request");
  const chips: { key: TimelineChipKey; label: string; count: number }[] = [
    { key: "all", label: "All", count: shown.length },
  ];
  for (const c of TIMELINE_CHIPS) {
    const count = shown.filter((e) => e.category === c.key).length;
    if (count > 0) chips.push({ key: c.key, label: c.label, count });
  }
  return chips;
}

/** One timeline event row. */
export interface TimelineEventRow {
  type: "event";
  key: string;
  event: InsightsEvent;
  tag: string;
  text: string;
  /** Start offset in ms from PHP start. */
  offsetMs: number | null;
  durMs: number | null;
}

/** A collapsed run of N+1 queries. */
export interface TimelineGroupRow {
  type: "group";
  key: string;
  issue: InsightsRequestIssue;
  tag: string;
  text: string;
  offsetMs: number | null;
  /** From the first event's start to the last event's end. */
  spanMs: number | null;
  totalMs: number;
  children: TimelineEventRow[];
}

/** The synthesised final row: status, size and content type. */
export interface TimelineDoneRow {
  type: "done";
  key: "done";
  tag: string;
  text: string;
  offsetMs: number;
}

/** A timeline row. */
export type TimelineRow = TimelineEventRow | TimelineGroupRow | TimelineDoneRow;

function asDumpEvent(e: InsightsEvent): DumpEvent {
  return { id: e.idx, category: e.category, ts_ms: 0, site: "", request_id: "", payload: e.payload };
}

function eventRow(e: InsightsEvent): TimelineEventRow {
  return {
    type: "event",
    key: `e${e.idx}`,
    event: e,
    tag: CATEGORY_LABEL[e.category],
    text: eventText(e),
    offsetMs: e.t_us == null ? null : e.t_us / 1000,
    durMs: e.dur_us == null ? null : e.dur_us / 1000,
  };
}

/** A grouped N+1 query's row: its bindings when captured, since the SQL is the
 *  same on every row of the group. */
function childRow(e: InsightsEvent): TimelineEventRow {
  const row = eventRow(e);
  const b = e.payload.bindings;
  return Array.isArray(b) && b.length > 0 ? { ...row, text: `bindings ${JSON.stringify(b)}` } : row;
}

/** One event's timeline text. Outgoing HTTP reads `METHOD url status`, cache
 *  `event key`. */
export function eventText(e: InsightsEvent): string {
  if (e.category === "http") {
    const p = e.payload;
    return [p.method, p.url, p.status].filter((v) => v !== undefined && v !== null && v !== "").join(" ");
  }
  if (e.category === "cache") {
    const ev = typeof e.payload.event === "string" ? `${e.payload.event} ` : "";
    return `${ev}${rowBody(asDumpEvent(e))}`.trim();
  }
  return rowBody(asDumpEvent(e)).replace(/\s+/g, " ");
}

/**
 * The drill-in timeline: every event in time order except the `request`
 * summary, with each N+1 issue's events collapsed into one group row at the
 * first event's place, then a synthesised `done` row at the request's end.
 */
export function groupTimeline(detail: InsightsRequestDetail): TimelineRow[] {
  const groupOf = new Map<number, InsightsRequestIssue>();
  for (const issue of detail.issues) {
    if (issue.kind !== "n_plus_one" || issue.event_indices.length < 2) continue;
    for (const idx of issue.event_indices) if (!groupOf.has(idx)) groupOf.set(idx, issue);
  }
  const byIdx = new Map(detail.events.map((e) => [e.idx, e]));
  const placed = new Set<string>();
  const rows: TimelineRow[] = [];
  const ordered = [...detail.events].sort((a, b) => (a.t_us ?? 0) - (b.t_us ?? 0) || a.idx - b.idx);
  for (const e of ordered) {
    if (e.category === "request") continue;
    const issue = groupOf.get(e.idx);
    if (!issue) {
      rows.push(eventRow(e));
      continue;
    }
    if (placed.has(issue.key)) continue;
    placed.add(issue.key);
    const children = issue.event_indices
      .map((i) => byIdx.get(i))
      .filter((x): x is InsightsEvent => !!x)
      .sort((a, b) => (a.t_us ?? 0) - (b.t_us ?? 0))
      .map(childRow);
    const startMs = children[0]?.offsetMs ?? null;
    const lastChild = children[children.length - 1];
    const endMs = lastChild?.offsetMs != null ? lastChild.offsetMs + (lastChild.durMs ?? 0) : null;
    rows.push({
      type: "group",
      key: `g${issue.key}`,
      issue,
      tag: `${issue.count}× N+1`,
      text: children[0]?.event.payload.sql != null ? String(children[0].event.payload.sql) : children[0]?.text ?? "",
      offsetMs: startMs,
      spanMs: startMs != null && endMs != null ? endMs - startMs : null,
      totalMs: issue.time_ms,
      children,
    });
  }
  const s = detail.summary;
  rows.push({
    type: "done",
    key: "done",
    tag: "DONE",
    text: [statusText(s.status), formatBytes(s.bytes), s.content_type?.split(";")[0] ?? ""].filter(Boolean).join(" · "),
    offsetMs: s.duration_ms,
  });
  return rows;
}

/** The rows a timeline chip keeps: everything for All, else the category's
 *  events and groups (N+1 groups are queries). */
export function filterTimeline(rows: readonly TimelineRow[], key: TimelineChipKey): TimelineRow[] {
  if (key === "all") return [...rows];
  return rows.filter((r) => (r.type === "event" ? r.event.category === key : r.type === "group" && key === "query"));
}

/** The request list row's sub-line, e.g. `11:03:12 · 31 queries · 1 dump`. */
export function requestSubline(r: InsightsRequestSummary): string {
  const parts = [formatClock(r.ts_ms)];
  if (r.cold) {
    parts.push("cold start, not in stats");
    return parts.join(" · ");
  }
  if (r.traced || r.queries) parts.push(plural(r.queries, "query", "queries"));
  if (r.dumps) parts.push(plural(r.dumps, "dump", "dumps"));
  if (r.logs) parts.push(plural(r.logs, "log", "logs"));
  if (r.aborted) parts.push("client went away");
  return parts.join(" · ");
}

/** The drill-in's meta line: route name, action and start time. */
export function requestMeta(r: InsightsRequestSummary): string {
  return [r.route_name, r.route_action, formatClock(r.ts_ms)].filter(Boolean).join(" · ");
}

/** Text colour classes per tone, for figures and sub-lines. */
export const TONE_TEXT: Record<ChipTone, string> = {
  neutral: "text-muted-foreground",
  brand: "text-brand",
  success: "text-success",
  warning: "text-warning",
  danger: "text-destructive",
};

/** One issue-detection threshold field, in the settings dialog's order. */
export interface ThresholdField {
  key: keyof InsightsThresholds;
  label: string;
  hint: string;
  unit: string;
  min: number;
}

/** The five issue-detection thresholds, with the daemon's lower bounds. */
export const THRESHOLD_FIELDS: readonly ThresholdField[] = [
  { key: "n_plus_one_min", label: "N+1 queries", hint: "Same query shape from one call site, at least", unit: "times", min: 1 },
  { key: "duplicate_min", label: "Duplicate queries", hint: "Identical SQL and bindings, at least", unit: "times", min: 2 },
  { key: "slow_query_ms", label: "Slow query", hint: "A single query slower than", unit: "ms", min: 1 },
  { key: "large_read_rows", label: "Large read", hint: "A read without LIMIT returning at least", unit: "rows", min: 1 },
  { key: "max_queries", label: "Too many queries", hint: "A request running more than", unit: "queries", min: 1 },
];

/** The threshold inputs as typed, keyed by field. */
export type ThresholdDraft = Record<keyof InsightsThresholds, string>;

/** The thresholds as editable strings. */
export function thresholdDraft(t: InsightsThresholds): ThresholdDraft {
  return {
    n_plus_one_min: String(t.n_plus_one_min),
    duplicate_min: String(t.duplicate_min),
    slow_query_ms: String(t.slow_query_ms),
    large_read_rows: String(t.large_read_rows),
    max_queries: String(t.max_queries),
  };
}

/**
 * Validate the typed thresholds: each must be a whole number at or above its
 * field's minimum (duplicates need at least 2, since one query is not a
 * duplicate). Returns the parsed thresholds, or an error per bad field.
 */
export function parseThresholds(
  draft: ThresholdDraft,
): { ok: true; thresholds: InsightsThresholds } | { ok: false; errors: Partial<Record<keyof InsightsThresholds, string>> } {
  const errors: Partial<Record<keyof InsightsThresholds, string>> = {};
  const out: Partial<InsightsThresholds> = {};
  for (const f of THRESHOLD_FIELDS) {
    const raw = draft[f.key].trim();
    const n = /^\d+$/.test(raw) ? Number(raw) : Number.NaN;
    if (!Number.isSafeInteger(n) || n < f.min || n > 4_294_967_295) {
      errors[f.key] = `Enter a whole number of ${f.min} or more`;
    } else {
      out[f.key] = n;
    }
  }
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return { ok: true, thresholds: out as InsightsThresholds };
}
