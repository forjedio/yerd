import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import LaravelDumpsView from "./LaravelDumpsView.vue";
import { resetResourceCache } from "@/composables/useResource";
import type { DumpExtStatus } from "@/ipc/types";

function stubIpc(extensions: DumpExtStatus[]) {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "dumps_status":
        return Promise.resolve({
          type: "dumps_status",
          enabled: true,
          port: 2304,
          running: true,
          persist: false,
          extensions,
          counts: {},
          features: {},
        });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: { unmount: () => void }[] = [];

async function mountView() {
  const wrapper = mount(LaravelDumpsView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

describe("LaravelDumpsView legacy handling", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("shows the legacy banner and Unsupported badge when a legacy version is present", async () => {
    stubIpc([
      { version: "7.4", present: false, legacy: true },
      { version: "8.4", present: true },
    ]);
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="dumps-legacy-banner"]').exists()).toBe(true);
    expect(wrapper.text()).toContain("Unsupported (no dumps)");
  });

  it("shows neither banner nor Unsupported badge when only supported versions are present", async () => {
    stubIpc([{ version: "8.4", present: true }]);
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="dumps-legacy-banner"]').exists()).toBe(false);
    expect(wrapper.text()).not.toContain("Unsupported (no dumps)");
  });
});
