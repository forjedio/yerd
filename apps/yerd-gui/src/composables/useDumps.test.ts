import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h } from "vue";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { type Dumps, useDumps } from "./useDumps";
import { useToast } from "@/composables/useToast";
import type { DumpCategory, DumpEvent } from "@/ipc/types";

function ev(id: number, request_id: string, category: DumpCategory = "dump"): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000, site: "codestash.test", request_id, payload: {} };
}

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

let pages: DumpEvent[][] = [];
let failing = new Set<string>();

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string) => {
    if (failing.has(cmd)) return Promise.reject(new Error("daemon unreachable"));
    switch (cmd) {
      case "list_dumps": {
        const events = pages.shift() ?? [];
        const latest = events.reduce((m, e) => Math.max(m, e.id), 0);
        return Promise.resolve({ type: "dumps", events, removed_ids: [], counts: COUNTS, latest_id: latest, min_live_id: 0 });
      }
      case "dumps_status":
        return Promise.resolve({
          type: "dumps_status",
          enabled: true,
          port: 2304,
          running: true,
          persist: false,
          extensions: [],
          counts: COUNTS,
          features: {},
        });
      default:
        return Promise.resolve({ type: "ok" });
    }
  });
}

const mounted: VueWrapper[] = [];

async function setup(): Promise<Dumps> {
  let dumps: Dumps | null = null;
  const Host = defineComponent({
    setup() {
      dumps = useDumps({ pollWhileHidden: true, follow: () => true });
      return () => h("div");
    },
  });
  mounted.push(mount(Host));
  await flushPromises();
  if (!dumps) throw new Error("dumps not created");
  return dumps;
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

beforeEach(() => {
  vi.useFakeTimers();
  invokeMock.mockReset();
  pages = [[ev(1, "a"), ev(2, "a")], [ev(3, "b")]];
  failing = new Set();
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  vi.useRealTimers();
});

describe("useDumps stream", () => {
  it("advances the cursor and follows the newest request", async () => {
    const dumps = await setup();
    expect(dumps.selectedKey.value).toBe("a");
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    const since = invokeMock.mock.calls.filter((c) => c[0] === "list_dumps").map((c) => c[1]);
    expect(since).toEqual([{ since: 0 }, { since: 2 }]);
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["b", "a"]);
    expect(dumps.selectedKey.value).toBe("b");
  });

  it("drops a page fetched before a clear", async () => {
    const dumps = await setup();
    const base = invokeMock.getMockImplementation();
    let resolvePage: (page: unknown) => void = () => undefined;
    invokeMock.mockImplementation((cmd: string, args: unknown) =>
      cmd === "list_dumps"
        ? new Promise((resolve) => {
            resolvePage = resolve;
          })
        : base?.(cmd, args),
    );
    await vi.advanceTimersByTimeAsync(750);
    await dumps.clear();
    resolvePage({ type: "dumps", events: [ev(3, "b")], removed_ids: [], counts: COUNTS, latest_id: 3, min_live_id: 1 });
    await flushPromises();
    expect(dumps.events.value).toEqual([]);
    expect(dumps.selectedKey.value).toBeNull();
  });
});

describe("useDumps actions", () => {
  it.each([
    ["set_dumps_enabled", (d: Dumps) => d.setEnabled(true), "Couldn't turn on interception"],
    ["set_dumps_persist", (d: Dumps) => d.setPersist(true), "Couldn't change Keep history"],
    ["set_dump_feature", (d: Dumps) => d.setFeature("cache", false), "Couldn't update Cache events"],
    ["clear_dumps", (d: Dumps) => d.clear(), "Couldn't clear dumps"],
  ])("%s failing toasts and frees busy", async (cmd, act, title) => {
    const dumps = await setup();
    failing.add(cmd);
    await act(dumps);
    expect(toastTitles()).toContain(title);
    expect(dumps.busy.value).toBeNull();
  });
});
