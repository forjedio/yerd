import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import CommandPalette from "./CommandPalette.vue";
import type { Command } from "@/lib/shortcuts/registry";

function cmd(id: string, title: string): Command {
  return {
    id,
    title,
    group: "Go to",
    chord: { mod: true, key: "k" },
    scopes: ["main"],
    inPalette: true,
    run: () => {},
  };
}

const COMMANDS = [cmd("nav:/sites", "Go to Sites"), cmd("nav:/php", "Go to PHP")];

function mountPalette(run = vi.fn()) {
  const wrapper = mount(CommandPalette, {
    props: { open: true, commands: COMMANDS, run },
    global: { stubs: { teleport: true } },
  });
  return { wrapper, run };
}

describe("CommandPalette", () => {
  it("lists palette commands and filters by query", async () => {
    const { wrapper } = mountPalette();
    expect(wrapper.findAll("li")).toHaveLength(2);

    await wrapper.find("input").setValue("php");
    const items = wrapper.findAll("li");
    expect(items).toHaveLength(1);
    expect(items[0]?.text()).toContain("Go to PHP");
  });

  it("runs the selected command on Enter and closes", async () => {
    const { wrapper, run } = mountPalette();
    await wrapper.find("input").trigger("keydown", { key: "Enter" });
    expect(run).toHaveBeenCalledWith(COMMANDS[0]);
    expect(wrapper.emitted("update:open")?.[0]).toEqual([false]);
  });

  it("moves the selection with the arrow keys", async () => {
    const { wrapper, run } = mountPalette();
    await wrapper.find("input").trigger("keydown", { key: "ArrowDown" });
    await wrapper.find("input").trigger("keydown", { key: "Enter" });
    expect(run).toHaveBeenCalledWith(COMMANDS[1]);
  });

  it("closes on Escape without running", async () => {
    const { wrapper, run } = mountPalette();
    await wrapper.find("input").trigger("keydown", { key: "Escape" });
    expect(run).not.toHaveBeenCalled();
    expect(wrapper.emitted("update:open")?.[0]).toEqual([false]);
  });

  it("shows an empty state when nothing matches", async () => {
    const { wrapper } = mountPalette();
    await wrapper.find("input").setValue("zzz");
    expect(wrapper.text()).toContain("No matching commands");
  });
});
