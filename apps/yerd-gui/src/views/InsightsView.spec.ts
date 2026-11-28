import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(() => Promise.resolve()),
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));
const routerPush = vi.fn();
vi.mock("vue-router", () => ({ useRouter: () => ({ push: routerPush }) }));

const storage = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, v),
  removeItem: (k: string) => void storage.delete(k),
  clear: () => storage.clear(),
});

import InsightsView from "./InsightsView.vue";
import { detailFixture, overviewFixture, requestsFixture } from "@/components/insights/fixtures";
import { resetResourceCache } from "@/composables/useResource";
import type { InsightsOverview } from "@/ipc/types";

const SITES = [
  { name: "acme", document_root: "/srv/acme", php: "8.5", secure: true, kind: "linked", framework: "laravel" },
  { name: "blog", document_root: "/srv/blog", php: "8.4", secure: true, kind: "linked" },
];

let recordingSites: string[] = ["acme"];
let overview: InsightsOverview = overviewFixture();

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_sites":
        return Promise.resolve({ type: "sites", sites: SITES });
      case "list_parked":
        return Promise.resolve({ type: "parked", paths: [] });
      case "list_groups":
        return Promise.resolve({ type: "groups", order: [], members: {} });
      case "insights_status":
        return Promise.resolve({
          type: "insights_status",
          sites: recordingSites,
          thresholds: { n_plus_one_min: 5, duplicate_min: 2, slow_query_ms: 100, large_read_rows: 1000, max_queries: 50 },
          capture_enabled: true,
          open_issues: 5,
        });
      case "insights_overview":
        return Promise.resolve({ type: "insights_overview", overview });
      case "list_insights_requests":
        return Promise.resolve({ type: "insights_requests", requests: requestsFixture() });
      case "get_insights_request":
        return Promise.resolve({ type: "insights_request", request: detailFixture() });
      default:
        return Promise.resolve({ type: "ok" });
    }
  });
}

function setWide(wide: boolean): void {
  globalThis.matchMedia = vi.fn(() => ({
    matches: wide,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof globalThis.matchMedia;
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(InsightsView, {
    attachTo: document.body,
    global: { stubs: { teleport: true, RouterLink: true, DialogPortal: Pass, AlertDialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function calls(cmd: string): unknown[][] {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  storage.clear();
  resetResourceCache();
  invokeMock.mockReset();
  recordingSites = ["acme"];
  overview = overviewFixture();
  setWide(true);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("InsightsView", () => {
  it("picks the recording site and renders the overview", async () => {
    const w = await mountView();
    expect(storage.get("yerd.insights.site")).toBe("acme");
    expect(w.get('[data-testid="site-picker"]').text()).toContain("acme.test");
    expect(w.get('[data-testid="recording-pill"]').text()).toContain("Recording · this site");
    expect(w.get('[data-kpi="requests"]').text()).toContain("1,846");
    expect(w.get('[data-kpi="queries"]').text()).toContain("avg");
    expect(w.get('[data-testid="time-breakdown"]').text()).toContain("Average warm request");
    expect(w.findAll("[data-issue]")).toHaveLength(5);
  });

  it("puts the toolbar in its own bar and labels the tabs", async () => {
    const w = await mountView();
    expect(w.get("header").find('[data-testid="insights-toolbar"]').exists()).toBe(false);
    expect(w.find('[data-testid="insights-toolbar-bar"] [data-testid="insights-toolbar"]').exists()).toBe(true);
    expect(w.get('nav [data-tab="live"]').text()).toBe("Live Feed");
    const pill = w.get('[data-testid="issues-tab-count"]');
    expect(pill.text()).toBe("5");
    expect(pill.classes()).toContain("rounded-full");
  });

  it("offers Start recording for a site that is off", async () => {
    recordingSites = [];
    overview = { ...overviewFixture(), kpis: { ...overviewFixture().kpis, requests: 0 } };
    const w = await mountView();
    expect(w.find('[data-testid="start-recording"]').exists()).toBe(true);
    await w.get('[data-testid="start-recording-button"]').trigger("click");
    await flushPromises();
    expect(calls("set_insights_enabled")[0]?.[1]).toEqual({ site: "acme", enabled: true });
  });

  it("shows the timing-only hint without extension data", async () => {
    overview = { ...overviewFixture(), ext_seen: false };
    const w = await mountView();
    expect(w.find('[data-testid="timing-only"]').exists()).toBe(true);
  });

  it("dismisses an issue through the client", async () => {
    const w = await mountView();
    await w.get('[data-issue="0000000000000003"] [data-testid="dismiss"]').trigger("click");
    await flushPromises();
    expect(calls("dismiss_insights_issue")[0]?.[1]).toEqual({ site: "acme", key: "0000000000000003", dismissed: true });
  });

  it("asks for the slowest ten when the Requests panel opens", async () => {
    const w = await mountView();
    await w.get('[data-testid="routes-panel"] [data-tab="requests"]').trigger("click");
    await flushPromises();
    expect(calls("list_insights_requests").slice(-1)[0]?.[1]).toMatchObject({ site: "acme", sort: "slowest", limit: 10 });
    expect(w.findAll('[data-testid="slow-row"]')).toHaveLength(8);
  });

  it("opens the latest affected request from an issue", async () => {
    const w = await mountView();
    await w.get('[data-issue="0000000000000001"] button').trigger("click");
    await flushPromises();
    expect(calls("get_insights_request")[0]?.[1]).toMatchObject({ site: "acme", requestId: "b".repeat(32) });
    expect(w.find('[data-testid="requests-tab"]').exists()).toBe(true);
    expect(w.get('[data-testid="insights-detail"]').text()).toContain("/orders/1042");
  });

  it("renders the request list as a table in the compact layout", async () => {
    setWide(false);
    const w = await mountView();
    await w.get('[data-tab="requests"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="insights-requests"]').attributes("data-layout")).toBe("table");
    expect(w.get('[data-testid="requests-tab"]').attributes("data-layout")).toBe("compact");
  });

  it("clears after confirming", async () => {
    const w = await mountView();
    await w.get('[data-testid="clear"]').trigger("click");
    await flushPromises();
    const confirm = w.findAll("button").find((b) => b.text().trim() === "Clear");
    await confirm?.trigger("click");
    await flushPromises();
    expect(calls("clear_insights")[0]?.[1]).toEqual({ site: "acme" });
  });

  it("saves thresholds from the settings dialog and closes it", async () => {
    const w = await mountView();
    await w.get('[data-testid="settings"]').trigger("click");
    await flushPromises();
    const slow = document.body.querySelector<HTMLInputElement>('[aria-label="Slow query"]');
    expect(slow?.value).toBe("100");
    if (slow) {
      slow.value = "250";
      slow.dispatchEvent(new Event("input"));
    }
    await flushPromises();
    document.body.querySelector<HTMLButtonElement>('[data-testid="save-thresholds"]')?.click();
    await flushPromises();
    expect(calls("set_insights_thresholds")[0]?.[1]).toEqual({
      thresholds: { n_plus_one_min: 5, duplicate_min: 2, slow_query_ms: 250, large_read_rows: 1000, max_queries: 50 },
    });
    expect(document.body.querySelector('[data-testid="save-thresholds"]')).toBeNull();
  });

  it("sends Change port to Settings", async () => {
    const w = await mountView();
    await w.get('[data-testid="settings"]').trigger("click");
    await flushPromises();
    const link = Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Change port");
    link?.click();
    await flushPromises();
    expect(routerPush).toHaveBeenCalledWith("/general");
  });
});
