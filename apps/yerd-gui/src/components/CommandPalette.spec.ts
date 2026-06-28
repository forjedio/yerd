import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import CommandPalette from "./CommandPalette.vue";
import type { Chord } from "@/lib/shortcuts/chord";
import type { Command } from "@/lib/shortcuts/registry";

function cmd(id: string, title: string, group: string, chord?: Chord): Command {
  return { id, title, group, chord, scopes: ["main"], inPalette: true, run: () => {} };
}

const STATIC = [
  cmd("nav:/sites", "Go to Sites", "Go to", { mod: true, code: "Digit3" }),
  cmd("settings", "Open Settings", "General", { mod: true, key: "," }),
];
const SITE = [
  cmd("site-open:zeta", "Open zeta.test", "zeta.test"),
  cmd("site-secure:zeta", "Secure zeta.test", "zeta.test"),
];
// flat order: known groups first (Go to, General), then site groups descending.
const FLAT = [...STATIC, ...SITE];

function mountPalette(commands = FLAT, run = vi.fn()) {
  const wrapper = mount(CommandPalette, {
    props: { open: true, commands, run },
    global: { stubs: { teleport: true } },
  });
  return { wrapper, run };
}

describe("CommandPalette", () => {
  it("renders grouped sections with headers", () => {
    const { wrapper } = mountPalette();
    const text = wrapper.text();
    expect(text).toContain("Go to");
    expect(text).toContain("zeta.test");
    expect(wrapper.findAll("li")).toHaveLength(4);
  });

  it("filters across groups by query", async () => {
    const { wrapper } = mountPalette();
    await wrapper.find("input").setValue("zeta");
    const items = wrapper.findAll("li");
    expect(items).toHaveLength(2);
    expect(wrapper.text()).toContain("zeta.test");
    expect(wrapper.text()).not.toContain("Go to Sites");
  });

  it("runs the first flat command on Enter", async () => {
    const { wrapper, run } = mountPalette();
    await wrapper.find("input").trigger("keydown", { key: "Enter" });
    expect(run).toHaveBeenCalledWith(STATIC[0]);
  });

  it("traverses across groups with the arrow keys", async () => {
    const { wrapper, run } = mountPalette();
    const input = wrapper.find("input");
    await input.trigger("keydown", { key: "ArrowDown" });
    await input.trigger("keydown", { key: "ArrowDown" });
    await input.trigger("keydown", { key: "Enter" });
    expect(run).toHaveBeenCalledWith(SITE[0]);
  });

  it("resets the selection when the command list grows asynchronously", async () => {
    const { wrapper, run } = mountPalette(STATIC);
    await wrapper.find("input").trigger("keydown", { key: "ArrowDown" });
    await wrapper.setProps({ commands: FLAT });
    await wrapper.find("input").trigger("keydown", { key: "Enter" });
    expect(run).toHaveBeenCalledWith(STATIC[0]);
  });

  it("closes on Escape without running", async () => {
    const { wrapper, run } = mountPalette();
    await wrapper.find("input").trigger("keydown", { key: "Escape" });
    expect(run).not.toHaveBeenCalled();
    expect(wrapper.emitted("update:open")?.[0]).toEqual([false]);
  });
});
