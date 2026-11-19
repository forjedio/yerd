import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
}));

import LaravelDumpsView from "./LaravelDumpsView.vue";
import { resetResourceCache } from "@/composables/useResource";
import type { DumpCategory, DumpEvent } from "@/ipc/types";

function ev(id: number, request_id: string, category: DumpCategory, payload: Record<string, unknown> = {}): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000, site: "codestash.test", request_id, payload };
}

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

let events: DumpEvent[] = [];

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_dumps":
        return Promise.resolve({ type: "dumps", events, removed_ids: [], counts: COUNTS, latest_id: events.length, min_live_id: 0 });
      case "dumps_status":
        return Promise.resolve({
          type: "dumps_status",
          enabled: true,
          port: 2304,
          running: true,
          persist: false,
          extensions: [
            { version: "8.4", present: true },
            { version: "7.4", present: false, legacy: true },
          ],
          counts: COUNTS,
          features: {},
        });
      case "show_dumps_window":
        return Promise.resolve(undefined);
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
  const wrapper = mount(LaravelDumpsView, {
    attachTo: document.body,
    global: { stubs: { teleport: true, RouterLink: true, DialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type View = Awaited<ReturnType<typeof mountView>>;

function detail(w: View) {
  return w.find('[data-testid="dumps-detail"]');
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

beforeEach(() => {
  invokeMock.mockReset();
  push.mockReset();
  resetResourceCache();
  events = [
    ev(1, "old", "query", { sql: "select 1" }),
    ev(2, "old", "request", { method: "get", uri: "/old", status: 200 }),
    ev(3, "new", "dump", { value_text: "hello" }),
    ev(4, "new", "request", { method: "post", uri: "/new", status: 302 }),
  ];
  setWide(true);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("LaravelDumpsView wide", () => {
  it("shows the list beside the newest request's detail", async () => {
    const w = await mountView();
    expect(w.get('[data-testid="dumps-requests"]').attributes("data-layout")).toBe("list");
    expect(w.get('[data-key="new"]').attributes("aria-current")).toBe("true");
    expect(w.get('[data-testid="detail-title"]').text()).toBe("/new");
    expect(w.get('[data-testid="capture-pill"]').text()).toBe("Intercepting on 2304");
  });

  it("keeps the selected detail while a filter it doesn't match narrows the list", async () => {
    const w = await mountView();
    await w.get('[aria-label="Filter events"]').trigger("click");
    await w.get('[data-testid="event-filter"]').setValue("select 1");
    expect(w.find('[data-key="new"]').exists()).toBe(false);
    expect(w.find('[data-key="old"]').exists()).toBe(true);
    expect(w.get('[data-testid="detail-title"]').text()).toBe("/new");
    expect(w.find('[data-testid="event-filter"]').exists()).toBe(true);
  });
});

describe("LaravelDumpsView compact", () => {
  beforeEach(() => setWide(false));

  it("opens a row in the drawer and closes it on Escape", async () => {
    const w = await mountView();
    expect(w.get('[data-testid="dumps-requests"]').attributes("data-layout")).toBe("table");
    expect(detail(w).exists()).toBe(false);
    await w.get('[data-key="old"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="detail-title"]').text()).toBe("/old");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flushPromises();
    expect(detail(w).exists()).toBe(false);
  });
});

describe("LaravelDumpsView actions", () => {
  it("pauses, opens the window, keeps history and clears through the daemon", async () => {
    const w = await mountView();
    await w.get('[data-testid="pause-resume"]').trigger("click");
    await flushPromises();
    await w.get('[aria-label="Open in window"]').trigger("click");
    await flushPromises();
    await w.findAll("button").find((b) => b.text() === "Keep history")?.trigger("click");
    await flushPromises();
    await w.get('[aria-label="Clear dumps"]').trigger("click");
    await flushPromises();
    expect(calls("set_dumps_enabled")).toEqual([["set_dumps_enabled", { enabled: false }]]);
    expect(calls("show_dumps_window")).toHaveLength(1);
    expect(calls("set_dumps_persist")).toEqual([["set_dumps_persist", { persist: true }]]);
    expect(calls("clear_dumps")).toHaveLength(1);
  });

  it("opens Setup with the legacy chip and sends Change port to Settings", async () => {
    const w = await mountView();
    await w.get('[aria-label="Setup"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("TOO OLD — NO DUMPS");
    const change = Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Change port");
    change?.click();
    await flushPromises();
    expect(push).toHaveBeenCalledWith("/general");
  });
});
