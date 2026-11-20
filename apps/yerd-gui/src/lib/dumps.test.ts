import { describe, expect, it } from "vitest";

import type { DumpCategory, DumpEvent, DumpsResponse, DumpsStatusResponse } from "@/ipc/types";
import {
  cacheTag,
  captureState,
  filterRequests,
  groupRequests,
  listDuration,
  matchesQuery,
  mergePage,
  metaLine,
  nextSelection,
  pillLabel,
  requestCategoryCounts,
  rowBody,
  rowCaller,
  rowCallerRel,
  rowDuration,
  shownCards,
  statusTag,
  streamRestarted,
  summaryMatches,
} from "./dumps";

function ev(
  id: number,
  category: DumpCategory,
  payload: Record<string, unknown> = {},
  request_id = "r1",
): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000 + id, site: "codestash.test", request_id, payload };
}

function page(overrides: Partial<DumpsResponse>): DumpsResponse {
  return {
    type: "dumps",
    events: [],
    removed_ids: [],
    counts: { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 },
    latest_id: 0,
    min_live_id: 0,
    ...overrides,
  };
}

function status(running: boolean, enabled: boolean): DumpsStatusResponse {
  return {
    type: "dumps_status",
    enabled,
    port: 2304,
    running,
    persist: false,
    extensions: [],
    counts: { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 },
    features: {},
  };
}

describe("streamRestarted", () => {
  it.each([
    ["the first page", 0, 0, false],
    ["an idle page", 5, 5, false],
    ["new events", 5, 9, false],
    ["ids that started over", 500, 3, true],
    ["a fresh daemon with nothing yet", 500, 0, true],
  ])("%s", (_, cursor, latest, expected) => {
    expect(streamRestarted(cursor, page({ latest_id: latest }))).toBe(expected);
  });
});

describe("mergePage", () => {
  it.each([
    ["appends unseen ids", [1, 2], page({ events: [ev(3, "dump")] }), [1, 2, 3]],
    ["drops removed ids", [1, 2, 3], page({ removed_ids: [2] }), [1, 3]],
    ["drops ids below min_live_id", [1, 2, 3], page({ min_live_id: 3 }), [3]],
    ["ignores ids already held", [1, 2], page({ events: [ev(2, "dump"), ev(4, "dump")] }), [1, 2, 4]],
  ])("%s", (_, held, p, ids) => {
    const merged = mergePage(
      held.map((id) => ev(id, "dump")),
      p,
    );
    expect(merged.map((e) => e.id)).toEqual(ids);
  });

  it("returns the held array itself when the page changes nothing", () => {
    const held = [ev(1, "dump"), ev(2, "dump")];
    expect(mergePage(held, page({ removed_ids: [9], min_live_id: 1 }))).toBe(held);
  });
});

describe("groupRequests", () => {
  it("groups interleaved events by request, newest request first", () => {
    const groups = groupRequests([
      ev(1, "query", {}, "a"),
      ev(2, "dump", {}, "b"),
      ev(3, "request", { method: "get", uri: "/a", status: 200, duration_ms: 12.5, ip: "127.0.0.1" }, "a"),
      ev(4, "log", {}, "b"),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["b", "a"]);
    expect(groups[1]?.events.map((e) => e.id)).toEqual([1, 3]);
    expect(groups[1]?.summary).toEqual({ method: "GET", uri: "/a", status: 200, durationMs: 12.5, ip: "127.0.0.1" });
    expect(groups[0]?.summary).toBeNull();
  });

  it("counts cards per chip without the summary", () => {
    const [req] = groupRequests([ev(1, "query"), ev(2, "query"), ev(3, "request", { uri: "/" })]);
    if (!req) throw new Error("no request");
    const counts = requestCategoryCounts(req);
    expect(counts.all).toBe(2);
    expect(counts.query).toBe(2);
    expect(counts.dump).toBe(0);
    expect(metaLine(req)).toMatch(/^codestash\.test · \d\d:\d\d:\d\d · 3 events$/);
  });
});

describe("filterRequests", () => {
  const requests = groupRequests([
    ev(1, "query", { sql: "select * from projects" }, "a"),
    ev(2, "dump", { value_text: "hello" }, "b"),
  ]);

  it.each([
    ["", "all", ["b", "a"]],
    ["projects", "all", ["a"]],
    ["", "query", ["a"]],
    ["hello", "query", []],
    ["projects", "dump", []],
  ] as const)("query %j in %s", (q, category, keys) => {
    expect(filterRequests(requests, q, category).map((r) => r.key)).toEqual(keys);
  });

  const withSummary = groupRequests([
    ev(1, "query", { sql: "select * from carts" }, "checkout"),
    ev(2, "request", { method: "post", uri: "/checkout", status: 500 }, "checkout"),
    ev(3, "dump", { value_text: "hello" }, "home"),
    ev(4, "request", { method: "get", uri: "/", status: 200 }, "home"),
    ev(5, "request", { method: "get", uri: "/ping", status: 204 }, "ping"),
  ]);

  it.each([
    ["/checkout", "all", ["checkout"]],
    ["POST", "all", ["checkout"]],
    ["500", "all", ["checkout"]],
    ["/ping", "all", ["ping"]],
    ["/checkout", "query", ["checkout"]],
    ["/checkout", "dump", []],
    ["/ping", "query", []],
    ["", "all", ["home", "checkout"]],
  ] as const)("matches the request summary: %j in %s", (q, category, keys) => {
    expect(filterRequests(withSummary, q, category).map((r) => r.key)).toEqual(keys);
  });

  it("shows every card of the chip when the summary matches, else the matching cards", () => {
    const checkout = withSummary.find((r) => r.key === "checkout");
    if (!checkout) throw new Error("no checkout request");
    expect(shownCards(checkout, "/checkout", "all").map((e) => e.id)).toEqual([1]);
    expect(shownCards(checkout, "carts", "all").map((e) => e.id)).toEqual([1]);
    expect(shownCards(checkout, "nothing", "all")).toEqual([]);
    expect(shownCards(checkout, "/checkout", "dump")).toEqual([]);
  });

  it("never matches a summary on an empty filter", () => {
    const checkout = withSummary.find((r) => r.key === "checkout");
    if (!checkout) throw new Error("no checkout request");
    expect(summaryMatches(checkout, "  ")).toBe(false);
  });

  it("matches on the caller", () => {
    expect(matchesQuery(ev(1, "dump", { file: "/srv/app/Foo.php", line: 3 }), "foo.php:3")).toBe(true);
  });
});

describe("event text and tags", () => {
  it.each([
    [200, "success"],
    [302, "warning"],
    [404, "danger"],
    [500, "danger"],
  ])("status %d is %s", (code, tone) => {
    expect(statusTag(ev(1, "http", { status: code }))?.tone).toBe(tone);
  });

  it.each([
    ["hit", "HIT", "success"],
    ["missed", "MISS", "danger"],
    ["written", "WRITE", "brand"],
    ["forgotten", "FORGET", "warning"],
    ["flushed", "FLUSHED", "neutral"],
  ])("cache %s reads %s", (event, text, tone) => {
    expect(cacheTag(ev(1, "cache", { event }))).toEqual({ text, tone });
  });

  it("joins a job's name, connection and status, dropping empty parts", () => {
    expect(rowBody(ev(1, "job", { name: "App\\Jobs\\Record", connection: "sync", status: "processed" }))).toBe(
      "App\\Jobs\\Record · sync · processed",
    );
    expect(rowBody(ev(1, "job", { name: "App\\Jobs\\Record", status: "queued" }))).toBe("App\\Jobs\\Record · queued");
  });

  it("shortens the caller and keeps the project-relative path", () => {
    const e = ev(1, "dump", { file: "/Users/me/Code/blog/app/Http/Foo.php", line: 31 });
    expect(rowCaller(e)).toBe("Foo.php:31");
    expect(rowCallerRel(e)).toBe("app/Http/Foo.php:31");
  });

  it("formats durations", () => {
    expect(rowDuration(ev(1, "query", { time_ms: 1.824 }))).toBe("1.82 ms");
    expect(rowDuration(ev(1, "http", { duration_ms: 1240 }))).toBe("1.24 s");
    expect(listDuration(186.4)).toBe("186 ms");
    expect(listDuration(1240)).toBe("1.24 s");
  });
});

describe("capture pill", () => {
  it.each([
    [null, "unknown", ""],
    [status(false, true), "stopped", "Dump server stopped"],
    [status(true, true), "live", "Intercepting on 2304"],
    [status(true, false), "paused", "Paused"],
  ] as const)("%j is %s", (s, state, label) => {
    expect(captureState(s)).toBe(state);
    expect(pillLabel(captureState(s), 2304, false)).toBe(label);
  });

  it("shortens the live label when compact", () => {
    expect(pillLabel("live", 2304, true)).toBe("Live · 2304");
  });
});

describe("nextSelection", () => {
  const two = groupRequests([ev(1, "dump", {}, "old"), ev(2, "dump", {}, "new")]);

  it.each([
    ["selects the newest on load", null, null, two, "new"],
    ["follows a newer arrival while the newest was selected", "old", "old", two, "new"],
    ["stays on an older pick", "new", "old", two, "old"],
    ["moves to the newest when the selection is gone", "new", "gone", two, "new"],
    ["clears on an empty list", "new", "new", [], null],
  ] as const)("%s", (_, prevFirst, selected, list, expected) => {
    expect(nextSelection(prevFirst, selected, list)).toBe(expected);
  });

  const three = groupRequests([ev(1, "dump", {}, "old"), ev(2, "dump", {}, "mid"), ev(3, "dump", {}, "hidden")]);
  const shownOnly = three.filter((r) => r.key !== "hidden");

  it.each([
    ["a hidden newer arrival never takes the selection", "mid", "mid", "mid"],
    ["a hidden selection that still exists stays put", "mid", "hidden", "hidden"],
    ["a gone selection moves to the newest shown", "mid", "gone", "mid"],
  ] as const)("%s", (_, prevFirst, selected, expected) => {
    expect(nextSelection(prevFirst, selected, shownOnly, three)).toBe(expected);
  });

  it("keeps a still-held selection while the filter shows nothing", () => {
    expect(nextSelection("mid", "mid", [], three)).toBe("mid");
  });
});
