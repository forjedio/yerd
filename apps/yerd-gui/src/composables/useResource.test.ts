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

    // Revisit: data is already cached, so loading must start false (no flash).
    const second = mountResource("greet", fetcher);
    expect(second.api.loading.value).toBe(false);
    expect(second.api.data.value).toBe("hi");
  });

  it("dedupes concurrent fetches for the same key", async () => {
    const fetcher = vi.fn().mockResolvedValue("x");
    mountResource("dup", fetcher);
    mountResource("dup", fetcher); // mounts before the first fetch settles
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
    expect(b.api.data.value).toEqual({ n: 42 }); // same shared ref
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
    expect(api.data.value).toBe("good"); // unchanged
    expect(api.error.value?.message).toBe("boom");
  });

  it("does not fetch on mount when immediate is false", async () => {
    const fetcher = vi.fn().mockResolvedValue("v");
    const { api } = mountResource("lazy", fetcher, { immediate: false });
    await flushPromises();
    expect(fetcher).not.toHaveBeenCalled();
    expect(api.data.value).toBeNull();

    await api.refresh();
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(api.data.value).toBe("v");
  });
});
