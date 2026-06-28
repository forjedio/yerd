import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// useResource only needs `IpcError` from the client; mock the Tauri core so the
// real client module loads without a Tauri runtime (mirrors client.test.ts).
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { IpcError } from "@/ipc/client";
import { invalidate, resetResourceCache, useResource } from "./useResource";

type Wrapper = ReturnType<typeof mount>;
let wrappers: Wrapper[] = [];

function mountResource<T>(
  key: string,
  fetcher: () => Promise<T>,
  opts?: { immediate?: boolean },
) {
  let api!: ReturnType<typeof useResource<T>>;
  const Comp = defineComponent({
    setup() {
      api = useResource(key, fetcher, opts);
      return () => h("div");
    },
  });
  const w = mount(Comp);
  wrappers.push(w);
  return { w, api };
}

beforeEach(() => resetResourceCache());
afterEach(() => {
  wrappers.forEach((w) => w.unmount());
  wrappers = [];
  vi.clearAllMocks();
});

describe("useResource", () => {
  it("shows loading on first load, then resolves with data", async () => {
    const fetcher = vi.fn().mockResolvedValue([1, 2, 3]);
    const { api } = mountResource("nums", fetcher);

    expect(api.loading.value).toBe(true);
    await flushPromises();
    expect(api.loading.value).toBe(false);
    expect(api.data.value).toEqual([1, 2, 3]);
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("renders a warm cache instantly without a spinner on revisit", async () => {
    const fetcher = vi.fn().mockResolvedValue("hi");
    const first = mountResource("greet", fetcher);
    await flushPromises();
    first.w.unmount();

    const second = mountResource("greet", fetcher);
    expect(second.api.loading.value).toBe(false);
    expect(second.api.data.value).toBe("hi");
  });

  it("dedupes concurrent fetches for the same key", async () => {
    const fetcher = vi.fn().mockResolvedValue("x");
    mountResource("dup", fetcher);
    mountResource("dup", fetcher);
    await flushPromises();
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("mutate writes the shared cache for every reader", async () => {
    const fetcher = vi.fn().mockResolvedValue({ n: 1 });
    const a = mountResource<{ n: number }>("obj", fetcher);
    const b = mountResource<{ n: number }>("obj", fetcher);
    await flushPromises();

    a.api.mutate((cur) => ({ n: (cur?.n ?? 0) + 41 }));
    expect(a.api.data.value).toEqual({ n: 42 });
    expect(b.api.data.value).toEqual({ n: 42 });
  });

  it("invalidate refetches the latest value", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce("old").mockResolvedValueOnce("new");
    const { api } = mountResource("inv", fetcher);
    await flushPromises();
    expect(api.data.value).toBe("old");

    await invalidate("inv");
    expect(api.data.value).toBe("new");
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("keeps last-good data when a revalidation fails", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce("good")
      .mockRejectedValueOnce(new IpcError("boom"));
    const { api } = mountResource("err", fetcher);
    await flushPromises();
    expect(api.data.value).toBe("good");

    await api.refresh();
    expect(api.data.value).toBe("good");
    expect(api.error.value?.message).toBe("boom");
  });

  it("does not fetch on mount when immediate is false", async () => {
    const fetcher = vi.fn().mockResolvedValue("v");
    const { api } = mountResource("lazy", fetcher, { immediate: false });
    expect(api.loading.value).toBe(false);
    await flushPromises();
    expect(fetcher).not.toHaveBeenCalled();
    expect(api.data.value).toBeNull();

    await api.refresh();
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(api.data.value).toBe("v");
  });

  it("a forced refresh chains a fresh fetch instead of deduping onto an in-flight one", async () => {
    let resolveFirst!: (v: string) => void;
    const fetcher = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<string>((res) => {
            resolveFirst = res;
          }),
      )
      .mockResolvedValueOnce("post-write");
    const { api } = mountResource("force", fetcher, { immediate: false });
    const inflight = api.refresh();
    const forced = api.refresh({ force: true });
    resolveFirst("pre-write");
    await Promise.all([inflight, forced]);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(api.data.value).toBe("post-write");
  });

  it("drives refreshing (not loading) on a warm revalidation", async () => {
    let resolveSecond!: (v: string) => void;
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce("first")
      .mockImplementationOnce(
        () =>
          new Promise<string>((res) => {
            resolveSecond = res;
          }),
      );
    const { api } = mountResource("warm", fetcher);
    await flushPromises();
    expect(api.data.value).toBe("first");

    const pending = api.refresh();
    expect(api.refreshing.value).toBe(true);
    expect(api.loading.value).toBe(false);
    resolveSecond("second");
    await pending;
    expect(api.refreshing.value).toBe(false);
    expect(api.data.value).toBe("second");
  });

  it("clears the error after a successful refresh following a failure", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce("good")
      .mockRejectedValueOnce(new IpcError("boom"))
      .mockResolvedValueOnce("recovered");
    const { api } = mountResource("recover", fetcher);
    await flushPromises();

    await api.refresh();
    expect(api.error.value?.message).toBe("boom");

    await api.refresh();
    expect(api.error.value).toBeNull();
    expect(api.data.value).toBe("recovered");
  });

  it("does not let an in-flight fetch clobber an optimistic mutate", async () => {
    let resolveFetch!: (v: number) => void;
    const fetcher = vi.fn(
      () =>
        new Promise<number>((res) => {
          resolveFetch = res;
        }),
    );
    const { api } = mountResource<number>("epoch", fetcher, { immediate: false });
    const pending = api.refresh();
    api.mutate(() => 99);
    expect(api.data.value).toBe(99);
    resolveFetch(1);
    await pending;
    expect(api.data.value).toBe(99);
  });

  it("shows the spinner again when retrying after a failed first load", async () => {
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new IpcError("down"))
      .mockResolvedValueOnce("ok");
    const { api } = mountResource("retry", fetcher);
    await flushPromises();
    expect(api.data.value).toBeNull();
    expect(api.error.value?.message).toBe("down");
    expect(api.loading.value).toBe(false);

    const pending = api.refresh();
    expect(api.loading.value).toBe(true);
    await pending;
    expect(api.loading.value).toBe(false);
    expect(api.data.value).toBe("ok");
  });
});
