import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, ref } from "vue";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { type Dumps, type DumpsOptions, useDumps } from "./useDumps";
import { useToast } from "@/composables/useToast";
import type { DumpCategory, DumpEvent } from "@/ipc/types";

function ev(id: number, request_id: string, category: DumpCategory = "dump"): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000, site: "codestash.test", request_id, payload: {} };
}

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

let pages: DumpEvent[][] = [];
let serverLatest = 0;
let failing = new Set<string>();

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string) => {
    if (failing.has(cmd)) return Promise.reject(new Error("daemon unreachable"));
    switch (cmd) {
      case "list_dumps": {
        const events = pages.shift() ?? [];
        serverLatest = events.reduce((m, e) => Math.max(m, e.id), serverLatest);
        return Promise.resolve({ type: "dumps", events, removed_ids: [], counts: COUNTS, latest_id: serverLatest, min_live_id: 0 });
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

async function setup(
  site?: () => string | null,
  extra: Pick<DumpsOptions, "tld" | "paused"> = {},
): Promise<Dumps> {
  let dumps: Dumps | null = null;
  const Host = defineComponent({
    setup() {
      dumps = useDumps({ pollWhileHidden: true, follow: () => true, site, ...extra });
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
  serverLatest = 0;
  failing = new Set();
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  vi.useRealTimers();
});

describe("useDumps site scope", () => {
  it("keeps only the scoped site's events and follows a site change", async () => {
    const scope = ref<string | null>("blog");
    pages = [[{ ...ev(1, "a"), site: "blog" }, { ...ev(2, "b"), site: "shop" }, { ...ev(3, "c"), site: "Blog" }]];
    const dumps = await setup(() => scope.value);
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["c", "a"]);
    scope.value = "shop";
    await flushPromises();
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["b"]);
    scope.value = null;
    await flushPromises();
    expect(dumps.requests.value).toHaveLength(3);
  });

  it("matches the host form older extensions send, under the given TLD only", async () => {
    pages = [
      [
        { ...ev(1, "a"), site: "blog.test" },
        { ...ev(2, "b"), site: "blog-api.test" },
        { ...ev(3, "c"), site: "blog.dev.local" },
        { ...ev(4, "d"), site: "blog" },
      ],
    ];
    const tld = ref("test");
    const dumps = await setup(() => "blog", { tld: () => tld.value });
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["d", "a"]);
    tld.value = "dev.local";
    await flushPromises();
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["d", "c"]);
  });
});

describe("useDumps pause", () => {
  it("stops streaming while paused and catches up on resume", async () => {
    const paused = ref(false);
    const dumps = await setup(undefined, { paused: () => paused.value });
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["a"]);
    paused.value = true;
    await flushPromises();
    await vi.advanceTimersByTimeAsync(1500);
    await flushPromises();
    expect(invokeMock.mock.calls.filter((c) => c[0] === "list_dumps")).toHaveLength(1);
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["a"]);
    paused.value = false;
    await flushPromises();
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["b", "a"]);
  });
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

  it("drops the held events and re-reads the stream after a daemon restart", async () => {
    pages = [[ev(480, "old-1"), ev(500, "old-2")]];
    const dumps = await setup();
    expect(dumps.selectedKey.value).toBe("old-2");
    serverLatest = 0;
    pages = [[], [ev(1, "new-1"), ev(2, "new-2")], [ev(3, "new-3")]];
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    const since = invokeMock.mock.calls.filter((c) => c[0] === "list_dumps").map((c) => c[1]);
    expect(since).toEqual([{ since: 0 }, { since: 500 }, { since: 0 }]);
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["new-2", "new-1"]);
    expect(dumps.selectedKey.value).toBe("new-2");
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(invokeMock.mock.calls.filter((c) => c[0] === "list_dumps").slice(-1)[0]?.[1]).toEqual({ since: 2 });
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["new-3", "new-2", "new-1"]);
  });

  it("follows the newest request the filter shows, not a hidden newer one", async () => {
    pages = [[ev(1, "a", "query")], [ev(2, "b", "dump")]];
    const dumps = await setup();
    dumps.category.value = "query";
    await flushPromises();
    expect(dumps.selectedKey.value).toBe("a");
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(dumps.requests.value.map((r) => r.key)).toEqual(["b", "a"]);
    expect(dumps.visible.value.map((r) => r.key)).toEqual(["a"]);
    expect(dumps.selectedKey.value).toBe("a");
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
