import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
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

function rowFor(wrapper: VueWrapper, name: string) {
  const row = wrapper.findAll("tbody tr").find((r) => r.text().includes(name));
  if (!row) throw new Error(`${name} row not rendered`);
  return row;
}

describe("ToolingView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    push.mockReset();
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("links an installed Node to the Node page instead of Update and Uninstall", async () => {
    const wrapper = await mountView([tool("node", true), tool("composer", true)]);
    const node = rowFor(wrapper, "Node.js");
    expect(node.text()).toContain("Manage versions");
    expect(node.text()).not.toContain("Update");
    expect(node.find('[aria-label="Uninstall Node.js"]').exists()).toBe(false);
    const manage = node.findAll("button").find((b) => b.text().includes("Manage versions"));
    await manage?.trigger("click");
    expect(push).toHaveBeenCalledWith("/node");
  });

  it("keeps Update and Uninstall for other installed tools", async () => {
    const wrapper = await mountView([tool("node", true), tool("composer", true)]);
    const composer = rowFor(wrapper, "composer");
    expect(composer.text()).toContain("Update");
    expect(composer.text()).not.toContain("Manage versions");
    expect(composer.find('[aria-label="Uninstall composer"]').exists()).toBe(true);
  });

  it("offers Install for Node when it is not installed", async () => {
    const wrapper = await mountView([tool("node", false)]);
    const node = rowFor(wrapper, "Node.js");
    expect(node.text()).toContain("Install");
    expect(node.text()).not.toContain("Manage versions");
  });
});
