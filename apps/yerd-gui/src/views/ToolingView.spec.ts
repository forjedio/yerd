import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import ToolingView from "./ToolingView.vue";
import type { ToolStatus } from "@/ipc/types";

function tool(id: string, installed: boolean): ToolStatus {
  return {
    id,
    display_name: id === "node" ? "Node.js" : id,
    installed,
    version: installed ? "1.0.0" : null,
    binaries: [id],
  };
}

const mounted: VueWrapper[] = [];

async function mountView(tools: ToolStatus[]) {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "list_tools") return Promise.resolve({ type: "tools", tools });
    return Promise.reject(new Error(`unexpected invoke ${cmd}`));
  });
  const wrapper = mount(ToolingView, { global: { stubs: { teleport: true } } });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

describe("ToolingView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("leaves Node to the Node page", async () => {
    for (const installed of [true, false]) {
      const wrapper = await mountView([tool("node", installed), tool("composer", true)]);
      const rows = wrapper.findAll("tbody tr").map((r) => r.text());
      expect(rows).toHaveLength(1);
      expect(rows[0]).toContain("composer");
      expect(wrapper.text()).not.toContain("Node.js runtime");
    }
  });

  it("keeps Update and Uninstall for other installed tools", async () => {
    const wrapper = await mountView([tool("node", true), tool("composer", true)]);
    const composer = wrapper.get("tbody tr");
    expect(composer.text()).toContain("Update");
    expect(composer.find('[aria-label="Uninstall composer"]').exists()).toBe(true);
  });
});
