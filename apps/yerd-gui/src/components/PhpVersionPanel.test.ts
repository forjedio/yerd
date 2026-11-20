import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import PhpVersionPanel from "./PhpVersionPanel.vue";
import { resetResourceCache, useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import type { PhpExtInfo } from "@/ipc/types";

const setPhpDirectives = vi.hoisted(() => vi.fn());
const setPhpPoolSettings = vi.hoisted(() => vi.fn());
const removePhpExtension = vi.hoisted(() => vi.fn());
const setPhpVersionSettings = vi.hoisted(() => vi.fn());
const setXdebug = vi.hoisted(() => vi.fn());
const listPhp = vi.hoisted(() => vi.fn());
const listPhpExtensions = vi.hoisted(() => vi.fn());
const openInBrowser = vi.hoisted(() => vi.fn());

// Vitest module mocks are total, so every client symbol the panel imports has
// to appear here or it arrives as undefined.
vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  listPhp,
  listPhpExtensions,
  openInBrowser,
  removePhpExtension,
  setPhpDirectives,
  setPhpPoolSettings,
  setPhpVersionSettings,
  setXdebug,
}));

// The row actions live in a reka-ui dropdown, which needs a real pointer stack
// to open; stub the parts so the items render inline.
const DropdownStub = { template: "<div><slot /></div>" };
const dropdownStubs = {
  DropdownMenu: DropdownStub,
  DropdownMenuTrigger: DropdownStub,
  DropdownMenuContent: DropdownStub,
  DropdownMenuItem: {
    template: '<div class="dropdown-item-stub" @click="$emit(\'select\')"><slot /></div>',
  },
  DropdownMenuSeparator: { template: "<hr />" },
};

const refreshed = {
  installed: ["8.3"],
  default: "8.3",
  directives: { "8.3": { "xdebug.mode": "off" } },
};

const XDEBUG: PhpExtInfo = {
  name: "xdebug",
  path: "/opt/homebrew/lib/php/pecl/20250925/xdebug.so",
  zend: true,
  present: true,
};

function mountPanel(
  props: Record<string, unknown> = {},
  options: { attachTo?: Element } = {},
) {
  return mount(PhpVersionPanel, {
    props: {
      version: "8.3",
      globalSettings: {},
      overrides: {},
      directives: { "xdebug.mode": "debug" },
      pool: {},
      extensions: [],
      installedVersion: true,
      extensionsLoading: false,
      ffiAvailable: false,
      libvipsFound: null,
      xdebug: null,
      xdebugAvailable: true,
      ...props,
    },
    global: { stubs: dropdownStubs },
    ...options,
  });
}

function button(w: ReturnType<typeof mountPanel>, label: string) {
  return w.find(`button[aria-label="${label}"]`);
}

/** The Save/Discard buttons carry no aria-label; find them by their text. */
function byText(w: ReturnType<typeof mountPanel>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

function saveButton(w: ReturnType<typeof mountPanel>) {
  return w.get('[data-testid="version-save"]');
}

async function revealAddRow(w: ReturnType<typeof mountPanel>) {
  await byText(w, "Add directive")!.trigger("click");
}

/** The most recent `dirty` payload (the ES2020 lib target rules out `.at`). */
function lastDirty(w: ReturnType<typeof mountPanel>) {
  const events = w.emitted("dirty");
  return events?.[events.length - 1];
}

describe("PhpVersionPanel directive editing", () => {
  beforeEach(() => {
    setPhpDirectives.mockReset();
    setPhpDirectives.mockResolvedValue(refreshed);
  });

  it("edits a directive's value in place and reports the refreshed list", async () => {
    const w = mountPanel();
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
    const w = mountPanel();
    await button(w, "Edit xdebug.mode").trigger("click");
    await w.find('input[aria-label="New value for xdebug.mode"]').setValue("off");
    await button(w, "Cancel editing xdebug.mode").trigger("click");

    expect(setPhpDirectives).not.toHaveBeenCalled();
    expect(w.text()).toContain("xdebug.mode = debug");
  });

  it("blocks saving an invalid or empty value", async () => {
    const w = mountPanel();
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

describe("PhpVersionPanel dirty state", () => {
  it("keeps Save disabled and Discard hidden until something changes", async () => {
    const w = mountPanel();
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeDefined();
    expect(byText(w, "Discard")).toBeUndefined();
    expect(lastDirty(w)).toEqual([false]);

    await w.find('input[id="set-8.3-memory_limit"]').setValue("512M");
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeUndefined();
    expect(byText(w, "Discard")!.attributes("disabled")).toBeUndefined();
    expect(lastDirty(w)).toEqual([true]);
  });

  it("counts a typed-but-unadded directive as unsaved work, enabling only Discard since the settings grid is unchanged", async () => {
    const w = mountPanel();
    await revealAddRow(w);
    await w.find('input[aria-label="Directive name for PHP 8.3"]').setValue("xdebug.");

    expect(lastDirty(w)).toEqual([true]);
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeDefined();
    expect(byText(w, "Discard")!.attributes("disabled")).toBeUndefined();
  });

  it("discards settings edits and pending directive input together", async () => {
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("512M");
    await revealAddRow(w);
    await w.find('input[aria-label="Directive name for PHP 8.3"]').setValue("xdebug.");

    await byText(w, "Discard")!.trigger("click");

    expect(
      (w.find('input[id="set-8.3-memory_limit"]').element as HTMLInputElement).value,
    ).toBe("");
    expect(w.find('input[aria-label="Directive name for PHP 8.3"]').exists()).toBe(false);
    expect(lastDirty(w)).toEqual([false]);
  });
});

describe("PhpVersionPanel FPM workers", () => {
  const poolInput = 'input[id="set-8.3-max_children"]';
  const refreshedPool = {
    installed: ["8.3"],
    default: "8.3",
    pool: { "8.3": { max_children: "32" } },
  };
  const refreshedSettings = {
    installed: ["8.3"],
    default: "8.3",
    version_settings: { "8.3": { memory_limit: "1G" } },
  };

  beforeEach(() => {
    setPhpPoolSettings.mockReset();
    setPhpPoolSettings.mockResolvedValue(refreshedPool);
    setPhpVersionSettings.mockReset();
    setPhpVersionSettings.mockResolvedValue(refreshedSettings);
  });

  it("shows the default as a placeholder when the version has no override", () => {
    const w = mountPanel();
    const input = w.find(poolInput).element as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("16");
  });

  it("seeds the field from an existing override", () => {
    const w = mountPanel({ pool: { max_children: "64" } });
    expect((w.find(poolInput).element as HTMLInputElement).value).toBe("64");
  });

  it("saves only the ceiling when only the ceiling changed", async () => {
    const w = mountPanel();
    await w.find(poolInput).setValue("32");
    await saveButton(w).trigger("click");
    await flushPromises();

    expect(setPhpPoolSettings).toHaveBeenCalledWith("8.3", { max_children: "32" });
    expect(setPhpVersionSettings).not.toHaveBeenCalled();
    expect(w.emitted("updated")?.[0]).toEqual([refreshedPool]);
    expect(useToast().toasts.value.map((t) => t.title)).toContain("PHP 8.3 settings saved");
  });

  it("saves the settings first and then the ceiling when both changed", async () => {
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("1G");
    await w.find(poolInput).setValue("32");
    await saveButton(w).trigger("click");
    await flushPromises();

    expect(setPhpVersionSettings).toHaveBeenCalledTimes(1);
    expect(setPhpPoolSettings).toHaveBeenCalledTimes(1);
    expect(setPhpVersionSettings.mock.invocationCallOrder[0]).toBeLessThan(
      setPhpPoolSettings.mock.invocationCallOrder[0],
    );
    expect(w.emitted("updated")).toEqual([[refreshedSettings], [refreshedPool]]);
    expect(lastDirty(w)).toEqual([false]);
  });

  it("sends nothing else and keeps everything staged when the settings fail", async () => {
    setPhpVersionSettings.mockRejectedValue(new Error("bad value"));
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("1G");
    await w.find(poolInput).setValue("32");
    await saveButton(w).trigger("click");
    await flushPromises();

    expect(setPhpPoolSettings).not.toHaveBeenCalled();
    expect(lastDirty(w)).toEqual([true]);
    expect((w.find(poolInput).element as HTMLInputElement).value).toBe("32");
    expect(useToast().toasts.value.map((t) => t.title)).toContain("Couldn't update PHP 8.3 settings");
  });

  it("says the other settings were saved when only the ceiling fails", async () => {
    setPhpPoolSettings.mockRejectedValue(new Error("pool busy"));
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("1G");
    await w.find(poolInput).setValue("32");
    await saveButton(w).trigger("click");
    await flushPromises();

    const toast = useToast().toasts.value.find(
      (t) => t.title === "Couldn't update the PHP 8.3 pool size",
    );
    expect(toast?.detail).toBe("The other settings were saved. pool busy");
    expect((w.find(poolInput).element as HTMLInputElement).value).toBe("32");
    expect(lastDirty(w)).toEqual([true]);
  });

  it("clearing the field resets to the default", async () => {
    setPhpPoolSettings.mockResolvedValue({ installed: ["8.3"], default: "8.3" });
    const w = mountPanel({ pool: { max_children: "64" } });
    await w.find(poolInput).setValue("");
    await saveButton(w).trigger("click");
    await flushPromises();

    expect(setPhpPoolSettings).toHaveBeenCalledWith("8.3", { max_children: "" });
  });

  it("blocks saving an out-of-range value and shows the problem", async () => {
    const w = mountPanel();
    for (const value of ["0", "2000"]) {
      await w.find(poolInput).setValue(value);
      expect(saveButton(w).attributes("disabled")).toBeDefined();
      expect(w.text()).toContain("between 1 and 1024");
    }
    expect(setPhpPoolSettings).not.toHaveBeenCalled();
  });

  it("counts a pending pool edit as unsaved work and discards it with the rest", async () => {
    const w = mountPanel();
    await w.find(poolInput).setValue("32");
    expect(lastDirty(w)).toEqual([true]);

    await byText(w, "Discard")!.trigger("click");
    expect((w.find(poolInput).element as HTMLInputElement).value).toBe("");
    expect(lastDirty(w)).toEqual([false]);
  });

  it("is hidden for a version that is no longer installed", () => {
    const w = mountPanel({ installedVersion: false, extensions: [XDEBUG] });
    expect(w.find(poolInput).exists()).toBe(false);
  });
});

// Every mutation here rewrites the same version's config and reseeds the form
// from the daemon's reply, so a second one landing mid-flight could clobber the
// first's result or discard edits made while it was away.
describe("PhpVersionPanel concurrent mutations", () => {
  beforeEach(() => {
    setPhpDirectives.mockReset();
    removePhpExtension.mockReset();
  });

  it("locks the whole panel while a request is in flight, not just the control that started it", async () => {
    let release: (v: unknown) => void = () => {};
    setPhpDirectives.mockReturnValue(new Promise((r) => (release = r)));

    const w = mountPanel({ extensions: [XDEBUG] });
    await button(w, "Remove xdebug.mode").trigger("click");
    await vi.waitFor(() => expect(setPhpDirectives).toHaveBeenCalled());

    expect(byText(w, "Add extension")!.attributes("disabled")).toBeDefined();
    expect(button(w, "Actions for xdebug").attributes("disabled")).toBeDefined();
    expect(byText(w, "Add directive")!.attributes("disabled")).toBeDefined();
    expect(w.find('input[id="set-8.3-memory_limit"]').attributes("disabled")).toBeDefined();

    release(refreshed);
    await vi.waitFor(() =>
      expect(byText(w, "Add extension")!.attributes("disabled")).toBeUndefined(),
    );
  });

  it("rejects a second mutation issued while one is already running", async () => {
    let release: (v: unknown) => void = () => {};
    setPhpDirectives.mockReturnValue(new Promise((r) => (release = r)));
    removePhpExtension.mockResolvedValue({ "8.3": [] });

    const w = mountPanel({ extensions: [XDEBUG] });
    await button(w, "Remove xdebug.mode").trigger("click");
    await vi.waitFor(() => expect(setPhpDirectives).toHaveBeenCalledTimes(1));

    const remove = w
      .findAll(".dropdown-item-stub")
      .find((i) => i.text().includes("Remove"));
    await remove!.trigger("click");

    expect(removePhpExtension).not.toHaveBeenCalled();
    release(refreshed);
  });
});

describe("PhpVersionPanel extensions", () => {
  beforeEach(() => {
    removePhpExtension.mockReset();
    removePhpExtension.mockResolvedValue({ "8.3": [] });
  });

  it("renders a registered extension and removes it", async () => {
    const w = mountPanel({ extensions: [XDEBUG] });
    expect(w.text()).toContain("xdebug");
    expect(w.text()).toContain("zend");

    const remove = w
      .findAll(".dropdown-item-stub")
      .find((i) => i.text().includes("Remove"));
    await remove!.trigger("click");
    await vi.waitFor(() => expect(removePhpExtension).toHaveBeenCalled());

    expect(removePhpExtension).toHaveBeenCalledWith("8.3", "xdebug");
    expect(w.emitted("extensionsUpdated")?.[0]).toEqual([{ "8.3": [] }]);
  });

  it("prefills and focuses the directive name from an extension without flagging the untouched value", async () => {
    const w = mountPanel({ extensions: [XDEBUG] }, { attachTo: document.body });
    const addDirective = w
      .findAll(".dropdown-item-stub")
      .find((i) => i.text().includes("Add ini directive"));
    await addDirective!.trigger("click");
    await nextTick();

    const field = w.find('input[aria-label="Directive name for PHP 8.3"]');
    expect((field.element as HTMLInputElement).value).toBe("xdebug.");
    expect(document.activeElement).toBe(field.element);
    expect(w.text()).not.toContain("enter a value");
  });

  it("flags an extension whose file has gone missing", () => {
    const w = mountPanel({ extensions: [{ ...XDEBUG, present: false }] });
    expect(w.text()).toContain("missing");
    expect(w.text()).toContain("moved or been deleted");
  });

  it("asks the view to open the add-extension modal", async () => {
    const w = mountPanel();
    await byText(w, "Add extension")!.trigger("click");
    expect(w.emitted("requestAddExtension")).toHaveLength(1);
  });
});

describe("PhpVersionPanel for an uninstalled version", () => {
  it("offers only extension removal, never settings or an add", () => {
    const w = mountPanel({ installedVersion: false, extensions: [XDEBUG] });

    expect(w.text()).toContain("no longer installed");
    expect(w.find('input[id="set-8.3-memory_limit"]').exists()).toBe(false);
    expect(w.find('input[aria-label="Directive name for PHP 8.3"]').exists()).toBe(false);
    expect(byText(w, "Add extension")).toBeUndefined();
    expect(
      w.findAll(".dropdown-item-stub").find((i) => i.text().includes("Remove")),
    ).toBeTruthy();
    expect(
      w
        .findAll(".dropdown-item-stub")
        .find((i) => i.text().includes("Add ini directive")),
    ).toBeUndefined();
  });
});

describe("PhpVersionPanel FFI override", () => {
  beforeEach(() => {
    setPhpVersionSettings.mockReset();
    setPhpVersionSettings.mockResolvedValue({ installed: ["8.3"], default: "8.3" });
  });

  const ffiSelect = 'select[aria-label="ffi.enable for PHP 8.3"]';

  it("hides the FFI select and leaves ffi.enable out of the save when the build lacks FFI", async () => {
    const w = mountPanel({ ffiAvailable: false, overrides: { "ffi.enable": "On" } });
    expect(w.find(ffiSelect).exists()).toBe(false);
    await w.find('input[id="set-8.3-memory_limit"]').setValue("1G");
    await byText(w, "Save changes")!.trigger("click");
    await nextTick();
    expect(setPhpVersionSettings).toHaveBeenCalledTimes(1);
    const payload = setPhpVersionSettings.mock.calls[0][1] as Record<string, string>;
    expect(payload.memory_limit).toBe("1G");
    expect("ffi.enable" in payload).toBe(false);
  });

  it("offers the FFI select when the build has FFI and sends the chosen override", async () => {
    const w = mountPanel({ ffiAvailable: true });
    const select = w.find(ffiSelect);
    expect(select.exists()).toBe(true);
    expect(select.text()).toContain("inherit (Off)");
    await select.setValue("On");
    await byText(w, "Save changes")!.trigger("click");
    await nextTick();
    const payload = setPhpVersionSettings.mock.calls[0][1] as Record<string, string>;
    expect(payload["ffi.enable"]).toBe("On");
  });

  it("shows the libvips hint only while FFI is effectively on and libvips is missing", async () => {
    const off = mountPanel({ ffiAvailable: true, libvipsFound: false });
    expect(off.find('[data-testid="libvips-hint"]').exists()).toBe(false);

    const inherited = mountPanel({
      ffiAvailable: true,
      libvipsFound: false,
      globalSettings: { "ffi.enable": "On" },
    });
    expect(inherited.find('[data-testid="libvips-hint"]').exists()).toBe(true);

    const found = mountPanel({
      ffiAvailable: true,
      libvipsFound: true,
      globalSettings: { "ffi.enable": "On" },
    });
    expect(found.find('[data-testid="libvips-hint"]').exists()).toBe(false);
  });
});

describe("PhpVersionPanel Xdebug", () => {
  const xdebugOn = {
    installed: ["8.3"],
    default: "8.3",
    xdebug: { "8.3": { enabled: true, modes: ["debug"], start: "always" } },
    xdebug_versions: ["8.3"],
  };

  function modeChip(w: ReturnType<typeof mountPanel>, mode: string) {
    return w.find(`button[aria-label="Xdebug ${mode} mode for PHP 8.3"]`);
  }

  function modeOn(w: ReturnType<typeof mountPanel>, mode: string): boolean {
    return modeChip(w, mode).attributes("aria-pressed") === "true";
  }

  async function toggleMode(w: ReturnType<typeof mountPanel>, mode: string) {
    await modeChip(w, mode).trigger("click");
  }

  function xdebugSwitch(w: ReturnType<typeof mountPanel>) {
    return w.find('button[aria-label="Xdebug for PHP 8.3"]');
  }

  function startSelect(w: ReturnType<typeof mountPanel>) {
    return w.find('select[aria-label="Xdebug start for PHP 8.3"]');
  }

  function xdebugApply(w: ReturnType<typeof mountPanel>) {
    return w.find('[data-testid="xdebug-apply"]');
  }

  beforeEach(() => {
    setXdebug.mockReset();
    setXdebug.mockResolvedValue(xdebugOn);
    listPhp.mockReset();
    listPhp.mockResolvedValue(xdebugOn);
    openInBrowser.mockReset();
  });

  it("turns Xdebug on immediately from the switch", async () => {
    const w = mountPanel();
    expect(xdebugSwitch(w).attributes("aria-checked")).toBe("false");
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", true);
    expect(w.emitted("updated")?.[0]).toEqual([xdebugOn]);
  });

  it("sends staged modes and start with the switch when turning on", async () => {
    const staged = {
      ...xdebugOn,
      xdebug: { "8.3": { enabled: true, modes: ["debug", "profile"], start: "trigger" } },
    };
    setXdebug.mockResolvedValue(staged);
    const w = mountPanel();
    await toggleMode(w, "profile");
    await w.find('select[aria-label="Xdebug start for PHP 8.3"]').setValue("trigger");
    expect(lastDirty(w)).toEqual([true]);
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledTimes(1);
    expect(setXdebug).toHaveBeenCalledWith("8.3", true, ["debug", "profile"], "trigger");
    expect(lastDirty(w)).toEqual([false]);
    expect(xdebugApply(w).attributes("disabled")).toBeDefined();
  });

  it("keeps staged edits and sends only the switch when turning off", async () => {
    setXdebug.mockResolvedValue({ ...xdebugOn, xdebug: {} });
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "trace");
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", false);
    expect(modeOn(w, "trace")).toBe(true);
    expect(lastDirty(w)).toEqual([true]);
  });

  it("turns Xdebug off from the switch when it is on", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    expect(xdebugSwitch(w).attributes("aria-checked")).toBe("true");
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", false);
  });

  it("seeds debug and always from the default when the version has no entry", () => {
    const w = mountPanel();
    expect(modeOn(w, "debug")).toBe(true);
    expect(modeOn(w, "profile")).toBe(false);
    const start = w.find('select[aria-label="Xdebug start for PHP 8.3"]');
    expect((start.element as HTMLSelectElement).value).toBe("always");
    expect(xdebugApply(w).attributes("disabled")).toBeDefined();
  });

  it("stages a mode change as unsaved work without calling the daemon", async () => {
    const w = mountPanel();
    await toggleMode(w, "profile");

    expect(lastDirty(w)).toEqual([true]);
    expect(xdebugApply(w).attributes("disabled")).toBeUndefined();
    expect(setXdebug).not.toHaveBeenCalled();
  });

  it("applies the staged modes and start together and keeps the saved values", async () => {
    const saved = {
      ...xdebugOn,
      xdebug: { "8.3": { enabled: true, modes: ["debug", "profile"], start: "trigger" } },
    };
    setXdebug.mockResolvedValue(saved);
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await startSelect(w).setValue("trigger");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", null, ["debug", "profile"], "trigger");
    expect(w.emitted("updated")?.[0]).toEqual([saved]);
    expect(lastDirty(w)).toEqual([false]);
    expect(modeOn(w, "profile")).toBe(true);
    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
  });

  it("applies only the modes when only the modes were edited", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", null, ["debug", "profile"], undefined);
  });

  it("applies only the start when only the start was edited", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await startSelect(w).setValue("trigger");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", null, undefined, "trigger");
  });

  it("sends only the edited start with the switch when turning on", async () => {
    const w = mountPanel();
    await startSelect(w).setValue("trigger");
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", true, undefined, "trigger");
  });

  it("refreshes an untouched field while another holds an edit", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await w.setProps({ xdebug: { enabled: true, modes: ["debug"], start: "trigger" } });

    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
    expect(modeOn(w, "profile")).toBe(true);

    await toggleMode(w, "profile");
    expect(lastDirty(w)).toEqual([false]);
    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
  });

  it("shows the latest saved modes when an edit is reverted after a refresh", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await w.setProps({ xdebug: { enabled: true, modes: ["debug", "trace"], start: "always" } });
    await toggleMode(w, "profile");

    expect(modeOn(w, "trace")).toBe(true);
    expect(modeOn(w, "profile")).toBe(false);
    expect(lastDirty(w)).toEqual([false]);
  });

  it("reloads the PHP state after a failed save before unlocking the controls", async () => {
    setXdebug.mockRejectedValue(new Error("daemon did not respond within 120s"));
    let finishRead: (r: typeof xdebugOn) => void = () => {};
    listPhp.mockReturnValue(
      new Promise((resolve) => {
        finishRead = resolve;
      }),
    );
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(listPhp).toHaveBeenCalled());
    await flushPromises();

    expect(xdebugApply(w).attributes("disabled")).toBeDefined();
    expect(w.emitted("updated")).toBeUndefined();

    finishRead(xdebugOn);
    await flushPromises();

    expect(w.emitted("updated")?.[0]).toEqual([xdebugOn]);
    expect(xdebugApply(w).attributes("disabled")).toBeUndefined();
  });

  it("unlocks the controls and says so when the recovery read never answers", async () => {
    vi.useFakeTimers();
    try {
      setXdebug.mockRejectedValue(new Error("daemon did not respond within 120s"));
      listPhp.mockReturnValue(new Promise(() => {}));
      const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
      await toggleMode(w, "profile");
      await xdebugApply(w).trigger("click");
      await flushPromises();

      expect(listPhp).toHaveBeenCalled();
      expect(xdebugApply(w).attributes("disabled")).toBeDefined();

      await vi.advanceTimersByTimeAsync(10_000);
      await flushPromises();

      expect(xdebugApply(w).attributes("disabled")).toBeUndefined();
      expect(w.emitted("updated")).toBeUndefined();
      expect(useToast().toasts.value.map((t) => t.title)).toContain(
        "Couldn't confirm PHP 8.3's saved Xdebug state",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("applies a recovery read that resolves after the timeout", async () => {
    vi.useFakeTimers();
    try {
      setXdebug.mockRejectedValue(new Error("daemon did not respond within 120s"));
      let finishRead: (r: typeof xdebugOn) => void = () => {};
      listPhp.mockReturnValue(
        new Promise((resolve) => {
          finishRead = resolve;
        }),
      );
      const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
      await toggleMode(w, "profile");
      await xdebugApply(w).trigger("click");
      await flushPromises();
      await vi.advanceTimersByTimeAsync(10_000);
      await flushPromises();

      expect(xdebugApply(w).attributes("disabled")).toBeUndefined();
      expect(w.emitted("updated")).toBeUndefined();

      finishRead(xdebugOn);
      await flushPromises();

      expect(w.emitted("updated")).toEqual([[xdebugOn]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops a late recovery read once another save has started", async () => {
    vi.useFakeTimers();
    try {
      setXdebug.mockRejectedValueOnce(new Error("daemon did not respond within 120s"));
      let finishRead: (r: typeof xdebugOn) => void = () => {};
      listPhp.mockReturnValue(
        new Promise((resolve) => {
          finishRead = resolve;
        }),
      );
      const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
      await toggleMode(w, "profile");
      await xdebugApply(w).trigger("click");
      await flushPromises();
      await vi.advanceTimersByTimeAsync(10_000);
      await flushPromises();

      const saved = {
        ...xdebugOn,
        xdebug: { "8.3": { enabled: true, modes: ["debug", "profile"], start: "always" } },
      };
      let finishRetry: (r: typeof saved) => void = () => {};
      setXdebug.mockReturnValueOnce(
        new Promise((resolve) => {
          finishRetry = resolve;
        }),
      );
      await xdebugApply(w).trigger("click");
      await flushPromises();
      finishRead(xdebugOn);
      await flushPromises();

      expect(w.emitted("updated")).toBeUndefined();

      finishRetry(saved);
      await flushPromises();

      expect(w.emitted("updated")).toEqual([[saved]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("says the state is unconfirmed when the recovery read fails", async () => {
    setXdebug.mockRejectedValue(new Error("daemon did not respond within 120s"));
    listPhp.mockRejectedValue(new Error("daemon unreachable"));
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await toggleMode(w, "profile");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(listPhp).toHaveBeenCalled());
    await flushPromises();

    expect(xdebugApply(w).attributes("disabled")).toBeUndefined();
    expect(w.emitted("updated")).toBeUndefined();
    expect(
      useToast().toasts.value.find(
        (t) => t.detail === "Yerd couldn't read the current settings. Refresh to try again.",
      )?.title,
    ).toBe("Couldn't confirm PHP 8.3's saved Xdebug state");
  });

  it("explains the managed directives while Xdebug is on", () => {
    expect(mountPanel().find('[data-testid="xdebug-directives-hint"]').exists()).toBe(false);
    const on = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    expect(on.find('[data-testid="xdebug-directives-hint"]').text()).toContain("opcache.jit");
  });

  it("won't let the last checked mode be unchecked", async () => {
    const w = mountPanel();
    const debug = modeChip(w, "debug");
    expect(debug.attributes("disabled")).toBeDefined();

    await toggleMode(w, "coverage");
    expect(debug.attributes("disabled")).toBeUndefined();
    await toggleMode(w, "debug");
    expect(modeChip(w, "coverage").attributes("disabled")).toBeDefined();
    expect(modeOn(w, "coverage")).toBe(true);
  });

  it("discards staged Xdebug edits with the rest of the panel", async () => {
    const w = mountPanel();
    await toggleMode(w, "trace");
    await byText(w, "Discard")!.trigger("click");

    expect(modeOn(w, "trace")).toBe(false);
    expect(lastDirty(w)).toEqual([false]);
  });

  it("shows not available and disables every control when no build exists", () => {
    const w = mountPanel({ xdebugAvailable: false });
    expect(w.find('[data-testid="xdebug-unavailable"]').text()).toContain(
      "Xdebug isn't published for this PHP version on this platform.",
    );
    expect(xdebugSwitch(w).attributes("disabled")).toBeDefined();
    expect(modeChip(w, "develop").attributes("disabled")).toBeDefined();
    expect(
      w.find('select[aria-label="Xdebug start for PHP 8.3"]').attributes("disabled"),
    ).toBeDefined();
    expect(xdebugApply(w).attributes("disabled")).toBeDefined();
    expect(w.find('[data-testid="xdebug-ide-hint"]').exists()).toBe(false);
  });

  it("still lets an enabled but unavailable version be switched off", () => {
    const w = mountPanel({ xdebugAvailable: false, xdebug: xdebugOn.xdebug["8.3"] });
    expect(xdebugSwitch(w).attributes("disabled")).toBeUndefined();
  });

  it("shows the IDE port hint and opens the guide", async () => {
    const w = mountPanel();
    const hint = w.find('[data-testid="xdebug-ide-hint"]');
    expect(hint.text()).toContain("IDE listens on port 9003");
    expect(hint.attributes("title")).toContain("xdebug.client_port");
    await hint.find("button").trigger("click");
    expect(openInBrowser).toHaveBeenCalledWith("https://yerd.app/guide/xdebug");
  });

  it("is hidden for a version that is no longer installed", () => {
    const w = mountPanel({ installedVersion: false, extensions: [XDEBUG] });
    expect(w.find('[data-testid="xdebug-section"]').exists()).toBe(false);
  });
});

describe("PhpVersionPanel settings grid", () => {
  const saved = { installed: ["8.3"], default: "8.3", version_settings: { "8.3": {} } };

  beforeEach(() => {
    setPhpVersionSettings.mockReset();
    setPhpVersionSettings.mockResolvedValue(saved);
    setPhpPoolSettings.mockReset();
    setPhpPoolSettings.mockResolvedValue({ installed: ["8.3"], default: "8.3" });
  });

  function changedRows(w: ReturnType<typeof mountPanel>) {
    return w.findAll('[data-changed="true"]').map((r) => r.attributes("data-row"));
  }

  function segment(w: ReturnType<typeof mountPanel>, label: string) {
    return w
      .get('[aria-label="Display errors for PHP 8.3"]')
      .findAll('[role="radio"]')
      .find((b) => b.text() === label);
  }

  it("marks changed rows and counts them, ignoring a hidden FFI override", async () => {
    const w = mountPanel({ overrides: { memory_limit: "1G", "ffi.enable": "On" } });
    expect(changedRows(w)).toEqual(["memory_limit"]);
    expect(w.get('[data-testid="changed-count"]').text()).toContain("1 changed for 8.3");

    await w.find('input[id="set-8.3-max_children"]').setValue("32");
    expect(changedRows(w)).toEqual(["memory_limit", "max_children"]);
    expect(w.get('[data-testid="changed-count"]').text()).toContain("2 changed for 8.3");
  });

  it("says the version matches the shared defaults when nothing is changed", () => {
    const w = mountPanel();
    expect(w.get('[data-testid="changed-count"]').text()).toBe("Same as shared");
    expect(w.find('[data-testid="reset-to-shared"]').exists()).toBe(false);
  });

  it("stages blanks with Reset to shared and sends them on save", async () => {
    const w = mountPanel({
      overrides: { memory_limit: "1G", display_errors: "On" },
      pool: { max_children: "32" },
    });
    await w.get('[data-testid="reset-to-shared"]').trigger("click");
    expect(changedRows(w)).toEqual([]);
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();

    const payload = setPhpVersionSettings.mock.calls[0][1] as Record<string, string>;
    expect(payload.memory_limit).toBe("");
    expect(payload.display_errors).toBe("");
    expect(setPhpPoolSettings).toHaveBeenCalledWith("8.3", { max_children: "" });
  });

  it("stages inherit when the inherited display_errors value is picked", async () => {
    const w = mountPanel({ globalSettings: { display_errors: "On" }, overrides: { display_errors: "Off" } });
    expect(segment(w, "Default")).toBeUndefined();
    await segment(w, "On")!.trigger("click");
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();
    expect((setPhpVersionSettings.mock.calls[0][1] as Record<string, string>).display_errors).toBe("");
  });

  it("stages an override when the other display_errors value is picked", async () => {
    const w = mountPanel({ globalSettings: { display_errors: "On" } });
    await segment(w, "Off")!.trigger("click");
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();
    expect((setPhpVersionSettings.mock.calls[0][1] as Record<string, string>).display_errors).toBe("Off");
  });

  it.each([
    ["Default", "On", ""],
    ["On", "", "On"],
    ["Off", "", "Off"],
  ])("offers Default, On and Off without an inherited value (%s)", async (label, seeded, expected) => {
    const w = mountPanel({ overrides: { display_errors: seeded } });
    await segment(w, label)!.trigger("click");
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();
    expect((setPhpVersionSettings.mock.calls[0][1] as Record<string, string>).display_errors).toBe(expected);
  });

  it("sends only the edited override, never the untouched ones", async () => {
    const w = mountPanel({ overrides: { memory_limit: "1G" } });
    await w.find('input[id="set-8.3-upload_max_filesize"]').setValue("64M");
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();
    expect(setPhpVersionSettings).toHaveBeenCalledWith("8.3", { upload_max_filesize: "64M" });
  });

  it("takes an out-of-band override while dirty and doesn't revert it on save", async () => {
    const w = mountPanel();
    await w.find('input[id="set-8.3-upload_max_filesize"]').setValue("64M");
    await w.setProps({ overrides: { memory_limit: "1G" } });
    expect((w.find('input[id="set-8.3-memory_limit"]').element as HTMLInputElement).value).toBe("1G");
    expect((w.find('input[id="set-8.3-upload_max_filesize"]').element as HTMLInputElement).value).toBe(
      "64M",
    );
    await w.get('[data-testid="version-save"]').trigger("click");
    await flushPromises();
    expect(setPhpVersionSettings).toHaveBeenCalledWith("8.3", { upload_max_filesize: "64M" });
  });

  it("writes the shared PHP cache even when the panel is gone before the reply", async () => {
    resetResourceCache();
    let finish: (r: unknown) => void = () => {};
    setPhpVersionSettings.mockReturnValue(new Promise((r) => (finish = r)));
    const cache = useResource("php", listPhp, { immediate: false });
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("1G");
    await w.get('[data-testid="version-save"]').trigger("click");
    w.unmount();

    const reply = { installed: ["8.3"], default: "8.3", version_settings: { "8.3": { memory_limit: "1G" } } };
    finish(reply);
    await flushPromises();
    expect(cache.data.value).toEqual(reply);
  });
});

describe("PhpVersionPanel extras", () => {
  it("reveals the add-directive row from Add directive", async () => {
    const w = mountPanel();
    expect(w.find('[data-testid="directive-add-row"]').exists()).toBe(false);
    await byText(w, "Add directive")!.trigger("click");
    expect(w.find('[data-testid="directive-add-row"]').exists()).toBe(true);
    await byText(w, "Cancel")!.trigger("click");
    expect(w.find('[data-testid="directive-add-row"]').exists()).toBe(false);
  });

  it("reports an open extension menu as an open layer", async () => {
    const MenuStub = {
      name: "ExtMenuStub",
      emits: ["update:open"],
      template: "<div><slot /></div>",
    };
    const w = mount(PhpVersionPanel, {
      props: {
        version: "8.3",
        globalSettings: {},
        overrides: {},
        directives: {},
        pool: {},
        extensions: [XDEBUG],
        installedVersion: true,
        extensionsLoading: false,
        ffiAvailable: false,
        libvipsFound: null,
        xdebug: null,
        xdebugAvailable: true,
      },
      global: { stubs: { ...dropdownStubs, DropdownMenu: MenuStub } },
    });
    const menu = w.findComponent({ name: "ExtMenuStub" });
    menu.vm.$emit("update:open", true);
    await nextTick();
    expect(w.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([true]);
    menu.vm.$emit("update:open", false);
    await nextTick();
    expect(w.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([false]);
  });

  it("drops an open menu's layer when its row or the panel goes away", async () => {
    const MenuStub = {
      name: "ExtMenuStub",
      emits: ["update:open"],
      template: "<div><slot /></div>",
    };
    const stubbed = mount(PhpVersionPanel, {
      props: {
        version: "8.3",
        globalSettings: {},
        overrides: {},
        directives: {},
        pool: {},
        extensions: [XDEBUG],
        installedVersion: true,
        extensionsLoading: false,
        ffiAvailable: false,
        libvipsFound: null,
        xdebug: null,
        xdebugAvailable: true,
      },
      global: { stubs: { ...dropdownStubs, DropdownMenu: MenuStub } },
    });
    stubbed.findComponent({ name: "ExtMenuStub" }).vm.$emit("update:open", true);
    await nextTick();
    expect(stubbed.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([true]);

    await stubbed.setProps({ extensions: [] });
    expect(stubbed.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([false]);

    await stubbed.setProps({ extensions: [XDEBUG] });
    stubbed.findComponent({ name: "ExtMenuStub" }).vm.$emit("update:open", true);
    await nextTick();
    expect(stubbed.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([true]);
    stubbed.unmount();
    expect(stubbed.emitted("update:layerOpen")?.slice(-1)[0]).toEqual([false]);
  });
});
