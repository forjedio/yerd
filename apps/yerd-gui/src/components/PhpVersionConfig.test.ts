import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import PhpVersionConfig from "./PhpVersionConfig.vue";

const setPhpDirectives = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  setPhpDirectives,
  setPhpVersionSettings: vi.fn(),
}));

const refreshed = {
  installed: ["8.3"],
  default: "8.3",
  directives: { "8.3": { "xdebug.mode": "off" } },
};

async function mountOpen() {
  const w = mount(PhpVersionConfig, {
    props: {
      version: "8.3",
      globalSettings: {},
      overrides: {},
      directives: { "xdebug.mode": "debug" },
    },
  });
  await w.find("button[aria-expanded]").trigger("click");
  return w;
}

function button(w: Awaited<ReturnType<typeof mountOpen>>, label: string) {
  return w.find(`button[aria-label="${label}"]`);
}

describe("PhpVersionConfig directive editing", () => {
  beforeEach(() => {
    setPhpDirectives.mockReset();
    setPhpDirectives.mockResolvedValue(refreshed);
  });

  it("edits a directive's value in place and reports the refreshed list", async () => {
    const w = await mountOpen();
    expect(w.text()).toContain("xdebug.mode = debug");

    await button(w, "Edit xdebug.mode").trigger("click");
    const input = w.find('input[aria-label="New value for xdebug.mode"]');
    expect((input.element as HTMLInputElement).value).toBe("debug");

    await input.setValue("off");
    await button(w, "Save xdebug.mode").trigger("click");
    await vi.waitFor(() => expect(setPhpDirectives).toHaveBeenCalled());

    expect(setPhpDirectives).toHaveBeenCalledWith("8.3", { "xdebug.mode": "off" });
    expect(w.emitted("updated")?.[0]).toEqual([refreshed]);
    expect(w.find('input[aria-label="New value for xdebug.mode"]').exists()).toBe(false);
  });

  it("cancels an edit without saving", async () => {
    const w = await mountOpen();
    await button(w, "Edit xdebug.mode").trigger("click");
    await w.find('input[aria-label="New value for xdebug.mode"]').setValue("off");
    await button(w, "Cancel editing xdebug.mode").trigger("click");

    expect(setPhpDirectives).not.toHaveBeenCalled();
    expect(w.text()).toContain("xdebug.mode = debug");
  });

  it("blocks saving an invalid or empty value", async () => {
    const w = await mountOpen();
    await button(w, "Edit xdebug.mode").trigger("click");
    const input = w.find('input[aria-label="New value for xdebug.mode"]');

    await input.setValue("a;b");
    expect(button(w, "Save xdebug.mode").attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("values can't contain");

    await input.setValue("");
    expect(button(w, "Save xdebug.mode").attributes("disabled")).toBeDefined();
    expect(setPhpDirectives).not.toHaveBeenCalled();
  });
});
