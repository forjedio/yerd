import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));

const setAlwaysOnTop = vi.fn();
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setAlwaysOnTop }),
}));

let siteListener: ((e: { payload: unknown }) => void) | null = null;
const listenMock = vi.fn((_name: string, cb: (e: { payload: unknown }) => void) => {
  siteListener = cb;
  return Promise.resolve(() => undefined);
});
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, cb: (e: { payload: unknown }) => void) => listenMock(name, cb),
  emit: vi.fn(() => Promise.resolve()),
}));

const storage = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, v),
  removeItem: (k: string) => void storage.delete(k),
  clear: () => storage.clear(),
});

import DumpsWindowView from "./DumpsWindowView.vue";
import type { DumpEvent } from "@/ipc/types";
import { getViewActions } from "@/lib/shortcuts/useViewActions";

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

const EVENTS: DumpEvent[] = [
  { id: 1, category: "dump", ts_ms: 1_700_000_000_000, site: "codestash.test", request_id: "a", payload: { value_text: "hi" } },
];

const TitleBarStub = {
  props: ["title"],
  template: '<div><span data-testid="window-title">{{ title }}</span><slot name="actions" /></div>',
};
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(DumpsWindowView, {
    attachTo: document.body,
    global: { stubs: { TitleBar: TitleBarStub } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  storage.clear();
  siteListener = null;
  listenMock.mockClear();
  invokeMock.mockReset();
  setAlwaysOnTop.mockReset();
  setAlwaysOnTop.mockResolvedValue(undefined);
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_dumps":
        return Promise.resolve({ type: "dumps", events: EVENTS, removed_ids: [], counts: COUNTS, latest_id: 1, min_live_id: 0 });
      case "dumps_status":
        return Promise.resolve({
          type: "dumps_status",
          enabled: true,
          port: 2304,
          running: true,
          persist: true,
          extensions: [],
          counts: COUNTS,
          features: {},
        });
      default:
        return Promise.resolve({ type: "ok" });
    }
  });
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("DumpsWindowView", () => {
  it("shows the list and the selected request, pins and cycles the category", async () => {
    const w = await mountView();
    expect(w.get('[data-key="a"]').attributes("aria-current")).toBe("true");
    expect(w.get('[data-testid="detail-title"]').text()).toBe("codestash.test");
    expect(w.get('[data-testid="capture-pill"]').text()).toBe("Live · 2304");
    await w.get('[aria-label="Keep window on top"]').trigger("click");
    await flushPromises();
    expect(setAlwaysOnTop).toHaveBeenCalledWith(true);
    getViewActions().nextTab?.();
    await flushPromises();
    expect(w.get('[data-chip="dump"]').attributes("aria-checked")).toBe("true");
  });
});

describe("DumpsWindowView site scope", () => {
  const SCOPED: DumpEvent[] = [
    { id: 1, category: "dump", ts_ms: 1_700_000_000_000, site: "blog", request_id: "a", payload: { value_text: "a" } },
    { id: 2, category: "dump", ts_ms: 1_700_000_000_001, site: "shop", request_id: "b", payload: { value_text: "b" } },
  ];

  beforeEach(() => {
    const base = invokeMock.getMockImplementation();
    invokeMock.mockImplementation((cmd: string, args: unknown) =>
      cmd === "list_dumps"
        ? Promise.resolve({ type: "dumps", events: SCOPED, removed_ids: [], counts: COUNTS, latest_id: 2, min_live_id: 0 })
        : base?.(cmd, args),
    );
  });

  it("starts on the stored site and follows the Insights site event", async () => {
    storage.set("yerd.insights.site", "blog");
    const w = await mountView();
    expect(w.get('[data-testid="window-title"]').text()).toBe("Live feed · blog");
    expect(w.find('[data-key="a"]').exists()).toBe(true);
    expect(w.find('[data-key="b"]').exists()).toBe(false);
    expect(listenMock).toHaveBeenCalledWith("yerd:insights-site-changed", expect.any(Function));
    siteListener?.({ payload: "shop" });
    await flushPromises();
    expect(w.get('[data-testid="window-title"]').text()).toBe("Live feed · shop");
    expect(w.find('[data-key="a"]').exists()).toBe(false);
    expect(w.find('[data-key="b"]').exists()).toBe(true);
  });

  it("applies a site stored before the listener was registered", async () => {
    storage.set("yerd.insights.site", "blog");
    listenMock.mockImplementationOnce((_name, cb) => {
      siteListener = cb;
      storage.set("yerd.insights.site", "shop");
      return Promise.resolve(() => undefined);
    });
    const w = await mountView();
    expect(w.get('[data-testid="window-title"]').text()).toBe("Live feed · shop");
    expect(w.find('[data-key="b"]').exists()).toBe(true);
    expect(w.find('[data-key="a"]').exists()).toBe(false);
  });

  it("matches the host form under the daemon's TLD", async () => {
    storage.set("yerd.insights.site", "blog");
    const hosts: DumpEvent[] = [
      { id: 1, category: "dump", ts_ms: 1_700_000_000_000, site: "blog.dev.local", request_id: "a", payload: {} },
      { id: 2, category: "dump", ts_ms: 1_700_000_000_001, site: "blog-api.dev.local", request_id: "b", payload: {} },
    ];
    const base = invokeMock.getMockImplementation();
    invokeMock.mockImplementation((cmd: string, args: unknown) => {
      if (cmd === "status") return Promise.resolve({ type: "status", report: { tld: "dev.local" } });
      if (cmd === "list_dumps") {
        return Promise.resolve({ type: "dumps", events: hosts, removed_ids: [], counts: COUNTS, latest_id: 2, min_live_id: 0 });
      }
      return base?.(cmd, args);
    });
    const w = await mountView();
    expect(w.find('[data-key="a"]').exists()).toBe(true);
    expect(w.find('[data-key="b"]').exists()).toBe(false);
  });

  it("has no global pause button", async () => {
    const w = await mountView();
    expect(w.find('[data-testid="pause-resume"]').exists()).toBe(false);
  });
});
