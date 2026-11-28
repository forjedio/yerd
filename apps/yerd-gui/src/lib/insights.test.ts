import { describe, expect, it } from "vitest";

import type {
  InsightsEvent,
  InsightsIssue,
  InsightsOverview,
  InsightsRequestDetail,
  InsightsRequestSummary,
} from "@/ipc/types";
import {
  THRESHOLD_FIELDS,
  breakdownBar,
  chartGeometry,
  filterTimeline,
  formatBytes,
  formatMs,
  groupTimeline,
  isRange,
  issueChips,
  issueDetail,
  issueFilter,
  parseThresholds,
  thresholdDraft,
  issueImpact,
  issueTitle,
  kpiTiles,
  rangeLabel,
  requestParts,
  requestSubline,
  splitMs,
  statusLine,
  statusText,
  timelineChips,
  timingOnly,
} from "./insights";

function overview(patch: Partial<InsightsOverview> = {}): InsightsOverview {
  return {
    range: "1h",
    kpis: {
      p50_ms: 68,
      p95_ms: 231,
      p50_delta_ms: -9,
      p95_spike_ms: 412,
      p95_spike_at_ms: new Date(2026, 0, 1, 10, 42).getTime(),
      requests: 1846,
      per_minute: 31,
      static_skipped: 412,
      failed_pct: 1.9,
      s5xx: 29,
      s4xx: 6,
      queries_avg: 14,
      queries_max: 212,
      queries_max_route: "GET /orders/{order}",
    },
    cold: 4,
    dropped_requests: 0,
    dropped_frames: 0,
    ext_seen: true,
    ext_available: true,
    series: [],
    breakdown: [],
    routes: [],
    queries: [],
    issues: [],
    ...patch,
  };
}

function issue(patch: Partial<InsightsIssue>): InsightsIssue {
  return {
    key: "00000000000000ff",
    kind: "n_plus_one",
    sql: "select * from items where order_id = ?",
    hint: "eager-load with ->with('items')",
    file: "/srv/acme/app/Http/Controllers/OrderController.php",
    line: 42,
    occurrences: 96,
    affected_requests: 2,
    impact_ms: 188,
    max_in_request: 48,
    ...patch,
  };
}

function summary(patch: Partial<InsightsRequestSummary> = {}): InsightsRequestSummary {
  return {
    id: "a".repeat(32),
    ts_ms: new Date(2026, 0, 1, 11, 2, 41).getTime(),
    method: "GET",
    path: "/orders/1042",
    route_label: "/orders/{order}",
    status: 200,
    duration_ms: 318,
    bytes: 49_357,
    content_type: "text/html; charset=UTF-8",
    traced: true,
    queries: 52,
    dumps: 2,
    logs: 0,
    ...patch,
  };
}

describe("formatting", () => {
  it.each([
    [68.4, { value: "68", unit: "ms" }],
    [1240, { value: "1.24", unit: "s" }],
    [0.4, { value: "0.4", unit: "ms" }],
    [undefined, { value: "-", unit: "" }],
  ])("splitMs(%s)", (ms, expected) => {
    expect(splitMs(ms)).toEqual(expected);
  });

  it.each([
    [512, "512 B"],
    [49_357, "48.2 KB"],
    [3 * 1024 * 1024, "3.0 MB"],
  ])("formatBytes(%i) is %s", (n, expected) => {
    expect(formatBytes(n)).toBe(expected);
  });

  it("formatMs joins value and unit", () => {
    expect(formatMs(92)).toBe("92 ms");
  });

  it("statusText adds a known reason", () => {
    expect(statusText(200)).toBe("200 OK");
    expect(statusText(299)).toBe("299");
  });

  it.each([
    ["5m", "previous 5 min"],
    ["1h", "previous hour"],
    ["24h", "previous 24 hours"],
  ] as const)("rangeLabel(%s)", (range, expected) => {
    expect(rangeLabel(range)).toBe(expected);
  });

  it("isRange accepts only wire ranges", () => {
    expect(isRange("1h")).toBe(true);
    expect(isRange("session")).toBe(false);
  });
});

describe("kpiTiles", () => {
  it("builds the five tiles from a populated overview", () => {
    const tiles = kpiTiles(overview());
    expect(tiles.map((t) => `${t.value}${t.unit}`)).toEqual(["68ms", "231ms", "1,846", "1.9%", "14avg"]);
    expect(tiles[0]).toMatchObject({ sub: "9 ms vs previous hour", subTone: "success", trend: "down" });
    expect(tiles[1]).toMatchObject({ sub: "412 ms · spike at 10:42", subTone: "warning" });
    expect(tiles[2]?.sub).toBe("31/min · 412 static skipped");
    expect(tiles[3]).toMatchObject({ sub: "29 × 5xx · 6 × 4xx", valueTone: "danger" });
    expect(tiles[4]).toMatchObject({ sub: "max 212 on GET /orders/{order}", subTone: "warning" });
  });

  it("degrades without warm or extension data", () => {
    const tiles = kpiTiles(
      overview({
        kpis: {
          requests: 0,
          per_minute: 0,
          static_skipped: 0,
          failed_pct: 0,
          s5xx: 0,
          s4xx: 0,
          queries_max: 0,
        },
      }),
    );
    expect(tiles.map((t) => t.value)).toEqual(["-", "-", "0", "-", "-"]);
    expect(tiles[0]?.trend).toBeNull();
    expect(tiles[4]?.sub).toBe("Needs the Yerd PHP extension");
  });

  it("marks a slower median red with an up arrow", () => {
    const o = overview();
    o.kpis.p50_delta_ms = 12;
    expect(kpiTiles(o)[0]).toMatchObject({ subTone: "danger", trend: "up" });
  });
});

describe("statusLine and timingOnly", () => {
  it("reports a clean recording", () => {
    expect(statusLine(overview())).toEqual({
      cold: "4 cold requests set aside (first hit after an idle worker spawn)",
      dropped: "Every request recorded, 0 samples dropped",
      lossy: false,
    });
  });

  it("reports drops", () => {
    const s = statusLine(overview({ cold: 0, dropped_requests: 1, dropped_frames: 3 }));
    expect(s).toEqual({ cold: null, dropped: "1 request, 3 frames dropped", lossy: true });
  });

  it.each([
    [{ ext_seen: false, ext_available: true }, true],
    [{ ext_seen: true, ext_available: true }, false],
    [{ ext_seen: false, ext_available: false }, false],
  ])("timingOnly(%o) is %s", (patch, expected) => {
    expect(timingOnly(overview(patch))).toBe(expected);
  });
});

describe("chartGeometry", () => {
  const series = [
    { t_ms: 0, count: 2, p50_ms: 50, p95_ms: 100, errors: 0 },
    { t_ms: 60_000, count: 2, p50_ms: 60, p95_ms: 200, errors: 1 },
    { t_ms: 120_000, count: 0, errors: 0 },
    { t_ms: 180_000, count: 1, p50_ms: 40, p95_ms: 80, errors: 0 },
  ];

  it("scales to the box and breaks runs on missing data", () => {
    const g = chartGeometry(series, 300, 100, 60_000);
    expect(g.yMax).toBe(250);
    expect(g.p50.match(/M/g)).toHaveLength(2);
    expect(g.p95.match(/Z/g)).toHaveLength(2);
    expect(g.p50.startsWith("M0,")).toBe(true);
    expect(g.errors).toEqual([{ x: 100, y: 9, count: 1 }]);
    expect(g.spikeX).toBe(100);
    expect(g.ticks).toHaveLength(5);
    expect(g.ticks[4]?.x).toBe(300);
  });

  it("is empty without data", () => {
    const g = chartGeometry([], 300, 100);
    expect(g.p50).toBe("");
    expect(g.p95).toBe("");
    expect(g.ticks).toEqual([]);
    expect(g.spikeX).toBeNull();
  });

  it("centres a single point", () => {
    const g = chartGeometry([series[0]!], 300, 100);
    expect(g.p50.startsWith("M150,")).toBe(true);
  });
});

describe("breakdownBar", () => {
  it("orders parts and keeps given shares", () => {
    const bar = breakdownBar([
      { part: "db", ms: 26, share: 0.38 },
      { part: "app", ms: 29, share: 0.42 },
      { part: "wait", ms: 0, share: 0 },
    ]);
    expect(bar.map((s) => [s.part, s.share])).toEqual([
      ["app", 0.42],
      ["db", 0.38],
    ]);
    expect(bar[0]?.label).toBe("PHP / application");
  });

  it("derives shares from milliseconds", () => {
    const bar = breakdownBar(requestParts(summary({ app_ms: 88, db_ms: 201, http_ms: 18, wait_ms: 7 })));
    expect(bar.map((s) => s.short)).toEqual(["App", "Database", "HTTP", "Queue wait"]);
    expect(bar[1]?.share).toBeCloseTo(201 / 314);
  });
});

describe("issues", () => {
  const issues = [
    issue({ key: "1" }),
    issue({ key: "2", kind: "duplicate" }),
    issue({ key: "3", kind: "slow_query", occurrences: 4, impact_ms: 400 }),
    issue({ key: "4", dismissed: true }),
  ];

  it.each([
    ["all", ["1", "2", "3"]],
    ["n_plus_one", ["1"]],
    ["slow_query", ["3"]],
    ["dismissed", ["4"]],
    ["large_read", []],
  ] as const)("issueFilter(%s)", (key, keys) => {
    expect(issueFilter(issues, key).map((i) => i.key)).toEqual(keys);
  });

  it("lists chips with counts, hiding empty kinds", () => {
    expect(issueChips(issues, "all").map((c) => `${c.key}:${c.count}`)).toEqual([
      "all:3",
      "n_plus_one:1",
      "duplicate:1",
      "slow_query:1",
      "dismissed:1",
    ]);
    expect(issueChips(issues, "large_read").some((c) => c.key === "large_read")).toBe(true);
  });

  it("describes an N+1", () => {
    const i = issues[0]!;
    expect(issueTitle(i)).toBe("select * from items where order_id = ?");
    expect(issueDetail(i)).toBe(
      "48× in one request · app/Http/Controllers/OrderController.php:42 · eager-load with ->with('items')",
    );
    expect(issueImpact(i)).toBe("+94 ms");
  });

  it("shows a slow query's mean and a too-many count", () => {
    expect(issueImpact(issues[2]!)).toBe("100 ms");
    expect(issueTitle(issue({ kind: "too_many_queries", max_in_request: 212 }))).toBe("212 queries in one request");
  });
});

describe("timeline", () => {
  const ev = (idx: number, category: InsightsEvent["category"], t: number, payload: Record<string, unknown>): InsightsEvent => ({
    idx,
    category,
    t_us: t,
    dur_us: 1000,
    payload,
  });
  const detail: InsightsRequestDetail = {
    summary: summary(),
    events: [
      ev(0, "cache", 3200, { event: "hit", key: "team:7:settings" }),
      ev(1, "query", 8900, { sql: "select * from orders where id = 1042" }),
      ev(2, "query", 21000, { sql: "select * from items where order_id = 1" }),
      ev(3, "query", 22000, { sql: "select * from items where order_id = ?", bindings: [2] }),
      ev(4, "query", 23000, { sql: "select * from items where order_id = 3" }),
      ev(5, "http", 186300, { method: "GET", url: "https://api.stripe.com/v1/charges", status: 200 }),
      ev(6, "request", 318000, { method: "GET" }),
    ],
    issues: [
      {
        kind: "n_plus_one",
        key: "00000000000000ff",
        event_indices: [2, 3, 4],
        count: 3,
        time_ms: 3,
        hint: "eager-load",
      },
    ],
  };

  it("groups N+1 events and synthesises the done row", () => {
    const rows = groupTimeline(detail);
    expect(rows.map((r) => r.type)).toEqual(["event", "event", "group", "event", "done"]);
    const group = rows[2];
    expect(group?.type === "group" && group.children.map((c) => c.event.idx)).toEqual([2, 3, 4]);
    expect(group?.type === "group" && group.tag).toBe("3× N+1");
    expect(group?.type === "group" && group.spanMs).toBe(3);
    expect(group?.type === "group" && group.children[1]?.text).toBe("bindings [2]");
    expect(rows[0]?.text).toBe("hit team:7:settings");
    expect(rows[3]?.text).toBe("GET https://api.stripe.com/v1/charges 200");
    expect(rows[4]).toMatchObject({ text: "200 OK · 48.2 KB · text/html", offsetMs: 318 });
  });

  it("counts chips without the request summary", () => {
    expect(timelineChips(detail.events).map((c) => `${c.key}:${c.count}`)).toEqual([
      "all:6",
      "query:4",
      "cache:1",
      "http:1",
    ]);
  });

  it("filters rows by chip, keeping groups under queries", () => {
    const rows = groupTimeline(detail);
    expect(filterTimeline(rows, "query").map((r) => r.type)).toEqual(["event", "group"]);
    expect(filterTimeline(rows, "cache")).toHaveLength(1);
    expect(filterTimeline(rows, "all")).toHaveLength(5);
  });
});

describe("requestSubline", () => {
  it("lists counts", () => {
    expect(requestSubline(summary({ ts_ms: new Date(2026, 0, 1, 11, 3, 12).getTime(), dumps: 1 }))).toBe(
      "11:03:12 · 52 queries · 1 dump",
    );
  });

  it("labels a cold request", () => {
    expect(requestSubline(summary({ cold: true }))).toBe("11:02:41 · cold start, not in stats");
  });
});

describe("parseThresholds", () => {
  const DEFAULTS = { n_plus_one_min: 5, duplicate_min: 2, slow_query_ms: 100, large_read_rows: 1000, max_queries: 50 };

  it("round-trips the defaults through the draft", () => {
    expect(parseThresholds(thresholdDraft(DEFAULTS))).toEqual({ ok: true, thresholds: DEFAULTS });
  });

  it("lists the five fields in the dialog's order", () => {
    expect(THRESHOLD_FIELDS.map((f) => f.key)).toEqual([
      "n_plus_one_min",
      "duplicate_min",
      "slow_query_ms",
      "large_read_rows",
      "max_queries",
    ]);
  });

  it.each([
    ["n_plus_one_min", "0"],
    ["duplicate_min", "1"],
    ["slow_query_ms", ""],
    ["large_read_rows", "2.5"],
    ["max_queries", "-3"],
    ["max_queries", "abc"],
    ["slow_query_ms", "4294967296"],
  ] as const)("rejects %s = %j", (key, value) => {
    const r = parseThresholds({ ...thresholdDraft(DEFAULTS), [key]: value });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(Object.keys(r.errors)).toEqual([key]);
  });

  it("accepts the lower bounds and trims whitespace", () => {
    const r = parseThresholds({
      n_plus_one_min: " 1 ",
      duplicate_min: "2",
      slow_query_ms: "1",
      large_read_rows: "1",
      max_queries: "1",
    });
    expect(r).toEqual({
      ok: true,
      thresholds: { n_plus_one_min: 1, duplicate_min: 2, slow_query_ms: 1, large_read_rows: 1, max_queries: 1 },
    });
  });
});
