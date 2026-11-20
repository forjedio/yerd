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

import DumpsWindowView from "./DumpsWindowView.vue";
import type { DumpEvent } from "@/ipc/types";
import { getViewActions } from "@/lib/shortcuts/useViewActions";

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

const EVENTS: DumpEvent[] = [
  { id: 1, category: "dump", ts_ms: 1_700_000_000_000, site: "codestash.test", request_id: "a", payload: { value_text: "hi" } },
];

const TitleBarStub = { template: '<div><slot name="actions" /></div>' };
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
