import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, nextTick } from "vue";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
const emitMock = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/api/event", () => ({
  emit: (...args: unknown[]) => emitMock(...(args as [])),
}));

import { INSIGHTS_POLL_MS, INSIGHTS_SITE_EVENT, INSIGHTS_SITE_KEY, type Insights, useInsights } from "./useInsights";
import type { InsightsOverview } from "@/ipc/types";

const OVERVIEW: InsightsOverview = {
  range: "1h",
  kpis: { requests: 1, per_minute: 0, static_skipped: 0, failed_pct: 0, s5xx: 0, s4xx: 0, queries_max: 0 },
  cold: 0,
  dropped_requests: 0,
  dropped_frames: 0,
  ext_seen: false,
  ext_available: true,
  series: [],
  breakdown: [],
  routes: [],
  queries: [],
  issues: [],
};

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "insights_status":
        return Promise.resolve({
          type: "insights_status",
          sites: ["blog"],
          thresholds: { n_plus_one_min: 5, duplicate_min: 2, slow_query_ms: 100, large_read_rows: 1000, max_queries: 50 },
          capture_enabled: true,
          open_issues: 0,
        });
      case "insights_overview":
        return Promise.resolve({ type: "insights_overview", overview: OVERVIEW });
      case "list_insights_requests":
        return Promise.resolve({ type: "insights_requests", requests: [] });
      case "get_insights_request":
        return Promise.resolve({
          type: "insights_request",
          request: { summary: { id: "r1" }, events: [], issues: [] },
        });
      default:
        return Promise.resolve({ type: "ok" });
    }
  });
}

const mounted: VueWrapper[] = [];
const storage = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, v),
  removeItem: (k: string) => void storage.delete(k),
  clear: () => storage.clear(),
});

async function setup(): Promise<Insights> {
  let insights: Insights | null = null;
  const Host = defineComponent({
    setup() {
      insights = useInsights();
      return () => h("div");
    },
  });
  mounted.push(mount(Host));
  await flushPromises();
  if (!insights) throw new Error("insights not created");
  return insights;
}

function calls(cmd: string): unknown[][] {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

async function advance(): Promise<void> {
  await vi.advanceTimersByTimeAsync(INSIGHTS_POLL_MS);
  await flushPromises();
}

beforeEach(() => {
  vi.useFakeTimers();
  invokeMock.mockReset();
  emitMock.mockClear();
  localStorage.clear();
  localStorage.setItem(INSIGHTS_SITE_KEY, "blog");
  stubIpc();
});

afterEach(() => {
  for (const w of mounted.splice(0)) w.unmount();
  vi.useRealTimers();
});

describe("useInsights", () => {
  it("restores the site and polls the overview on the Overview tab", async () => {
    const i = await setup();
    expect(i.site.value).toBe("blog");
    expect(i.recording()).toBe(true);
    expect(i.overview.value).toEqual(OVERVIEW);
    expect(calls("insights_overview")).toHaveLength(1);
    expect(calls("list_insights_requests")).toHaveLength(0);
    await advance();
    expect(calls("insights_overview")).toHaveLength(2);
  });

  it("polls the request list only on the Requests tab", async () => {
    const i = await setup();
    i.tab.value = "requests";
    await nextTick();
    await flushPromises();
    const before = calls("insights_overview").length;
    expect(calls("list_insights_requests")).toHaveLength(1);
    expect(calls("list_insights_requests")[0]?.[1]).toMatchObject({ site: "blog", sort: "slowest" });
    await advance();
    expect(calls("list_insights_requests")).toHaveLength(2);
    expect(calls("insights_overview")).toHaveLength(before);
    expect(i.requestsLoaded.value).toBe(true);
  });

  it("fetches the slowest ten when the Overview's Requests panel is open", async () => {
    const i = await setup();
    i.routesTab.value = "requests";
    await nextTick();
    await flushPromises();
    expect(calls("list_insights_requests").slice(-1)[0]?.[1]).toMatchObject({ sort: "slowest", limit: 10 });
  });

  it("polls nothing but status on the Live feed tab", async () => {
    const i = await setup();
    i.tab.value = "live";
    await nextTick();
    await flushPromises();
    invokeMock.mockClear();
    await advance();
    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual(["insights_status"]);
  });

  it("stops polling while paused and resumes after", async () => {
    const i = await setup();
    i.paused.value = true;
    await nextTick();
    invokeMock.mockClear();
    await advance();
    await advance();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(i.overview.value).toEqual(OVERVIEW);
    i.paused.value = false;
    await nextTick();
    await flushPromises();
    expect(calls("insights_overview")).toHaveLength(1);
  });

  it("a site change resets the selection, persists and emits", async () => {
    const i = await setup();
    await i.select({ id: "r1", ts_ms: 5 });
    expect(i.detail.value?.summary.id).toBe("r1");
    expect(calls("get_insights_request")[0]?.[1]).toEqual({ site: "blog", requestId: "r1", tsMs: 5 });
    i.setSite("shop");
    expect(i.selectedId.value).toBeNull();
    expect(i.detail.value).toBeNull();
    expect(i.overview.value).toBeNull();
    expect(localStorage.getItem(INSIGHTS_SITE_KEY)).toBe("shop");
    expect(emitMock).toHaveBeenCalledWith(INSIGHTS_SITE_EVENT, "shop");
    await flushPromises();
    expect(calls("insights_overview").slice(-1)[0]?.[1]).toEqual({ site: "shop", range: "1h" });
    expect(i.recording()).toBe(false);
  });

  it("actions call the client for the selected site", async () => {
    const i = await setup();
    await i.setEnabled(false);
    await i.dismiss("00000000000000ff", true);
    await i.clear();
    expect(calls("set_insights_enabled")[0]?.[1]).toEqual({ site: "blog", enabled: false });
    expect(calls("dismiss_insights_issue")[0]?.[1]).toEqual({ site: "blog", key: "00000000000000ff", dismissed: true });
    expect(calls("clear_insights")[0]?.[1]).toEqual({ site: "blog" });
  });
});
