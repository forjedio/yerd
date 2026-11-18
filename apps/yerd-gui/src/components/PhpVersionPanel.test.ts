import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import PhpVersionPanel from "./PhpVersionPanel.vue";
import { useToast } from "@/composables/useToast";
import type { PhpExtInfo } from "@/ipc/types";

const setPhpDirectives = vi.hoisted(() => vi.fn());
const setPhpPoolSettings = vi.hoisted(() => vi.fn());
const removePhpExtension = vi.hoisted(() => vi.fn());
const setPhpVersionSettings = vi.hoisted(() => vi.fn());
const setXdebug = vi.hoisted(() => vi.fn());
const listPhp = vi.hoisted(() => vi.fn());
const openInBrowser = vi.hoisted(() => vi.fn());

// Vitest module mocks are total, so every client symbol the panel imports has
// to appear here or it arrives as undefined.
vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  listPhp,
  openInBrowser,
  removePhpExtension,
  setPhpDirectives,
  setPhpPoolSettings,
  setPhpVersionSettings,
  setXdebug,
}));

// The row actions live in a reka-ui dropdown, which needs a real pointer stack
// to open; stub the parts so the items render inline (as SiteCard.spec does).
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

/** The pool's Apply button; the Xdebug section above it has its own. */
function poolApply(w: ReturnType<typeof mountPanel>) {
  return w
    .find('[data-testid="pool-section"]')
    .findAll("button")
    .find((b) => b.text().includes("Apply"));
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
  it("keeps Save and Discard disabled until something changes", async () => {
    const w = mountPanel();
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeDefined();
    expect(byText(w, "Discard")!.attributes("disabled")).toBeDefined();
    expect(lastDirty(w)).toEqual([false]);

    await w.find('input[id="set-8.3-memory_limit"]').setValue("512M");
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeUndefined();
    expect(lastDirty(w)).toEqual([true]);
  });

  it("counts a typed-but-unadded directive as unsaved work, enabling only Discard since the settings grid is unchanged", async () => {
    const w = mountPanel();
    await w.find('input[aria-label="Directive name for PHP 8.3"]').setValue("xdebug.");

    expect(lastDirty(w)).toEqual([true]);
    expect(byText(w, "Save changes")!.attributes("disabled")).toBeDefined();
    expect(byText(w, "Discard")!.attributes("disabled")).toBeUndefined();
  });

  it("discards settings edits and pending directive input together", async () => {
    const w = mountPanel();
    await w.find('input[id="set-8.3-memory_limit"]').setValue("512M");
    await w.find('input[aria-label="Directive name for PHP 8.3"]').setValue("xdebug.");

    await byText(w, "Discard")!.trigger("click");

    expect(
      (w.find('input[id="set-8.3-memory_limit"]').element as HTMLInputElement).value,
    ).toBe("");
    expect(
      (
        w.find('input[aria-label="Directive name for PHP 8.3"]')
          .element as HTMLInputElement
      ).value,
    ).toBe("");
    expect(lastDirty(w)).toEqual([false]);
  });
});

describe("PhpVersionPanel FPM pool size", () => {
  const poolInput = 'input[id="pool-8.3-max-children"]';
  const refreshedPool = {
    installed: ["8.3"],
    default: "8.3",
    pool: { "8.3": { max_children: "32" } },
  };

  beforeEach(() => {
    setPhpPoolSettings.mockReset();
    setPhpPoolSettings.mockResolvedValue(refreshedPool);
  });

  it("shows the default as a placeholder when the version has no override", () => {
    const w = mountPanel();
    const input = w.find(poolInput).element as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("16 (default)");
  });

  it("seeds the field from an existing override", () => {
    const w = mountPanel({ pool: { max_children: "64" } });
    expect((w.find(poolInput).element as HTMLInputElement).value).toBe("64");
  });

  it("sets a pool value and reports the refreshed list", async () => {
    const w = mountPanel();
    await w.find(poolInput).setValue("32");
    await poolApply(w)!.trigger("click");
    await vi.waitFor(() => expect(setPhpPoolSettings).toHaveBeenCalled());

    expect(setPhpPoolSettings).toHaveBeenCalledWith("8.3", { max_children: "32" });
    expect(w.emitted("updated")?.[0]).toEqual([refreshedPool]);
  });

  it("clearing the field resets to the default", async () => {
    setPhpPoolSettings.mockResolvedValue({ installed: ["8.3"], default: "8.3" });
    const w = mountPanel({ pool: { max_children: "64" } });
    await w.find(poolInput).setValue("");
    await poolApply(w)!.trigger("click");
    await vi.waitFor(() => expect(setPhpPoolSettings).toHaveBeenCalled());

    expect(setPhpPoolSettings).toHaveBeenCalledWith("8.3", { max_children: "" });
  });

  it("blocks saving an out-of-range value", async () => {
    const w = mountPanel();
    for (const value of ["0", "2000"]) {
      await w.find(poolInput).setValue(value);
      expect(poolApply(w)!.attributes("disabled")).toBeDefined();
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

    expect(byText(w, "Add…")!.attributes("disabled")).toBeDefined();
    expect(button(w, "Actions for xdebug").attributes("disabled")).toBeDefined();
    expect(
      w.find('input[aria-label="Directive name for PHP 8.3"]').attributes("disabled"),
    ).toBeDefined();
    expect(w.find('input[id="set-8.3-memory_limit"]').attributes("disabled")).toBeDefined();

    release(refreshed);
    await vi.waitFor(() =>
      expect(byText(w, "Add…")!.attributes("disabled")).toBeUndefined(),
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
    await byText(w, "Add…")!.trigger("click");
    expect(w.emitted("requestAddExtension")).toHaveLength(1);
  });
});

describe("PhpVersionPanel for an uninstalled version", () => {
  it("offers only extension removal, never settings or an add", () => {
    const w = mountPanel({ installedVersion: false, extensions: [XDEBUG] });

    expect(w.text()).toContain("no longer installed");
    expect(w.find('input[id="set-8.3-memory_limit"]').exists()).toBe(false);
    expect(w.find('input[aria-label="Directive name for PHP 8.3"]').exists()).toBe(false);
    expect(byText(w, "Add…")).toBeUndefined();
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

  function modeBox(w: ReturnType<typeof mountPanel>, mode: string) {
    return w.find(`input[aria-label="Xdebug ${mode} mode for PHP 8.3"]`);
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
    await modeBox(w, "profile").setValue(true);
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
    await modeBox(w, "trace").setValue(true);
    await xdebugSwitch(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", false);
    expect((modeBox(w, "trace").element as HTMLInputElement).checked).toBe(true);
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
    expect((modeBox(w, "debug").element as HTMLInputElement).checked).toBe(true);
    expect((modeBox(w, "profile").element as HTMLInputElement).checked).toBe(false);
    const start = w.find('select[aria-label="Xdebug start for PHP 8.3"]');
    expect((start.element as HTMLSelectElement).value).toBe("always");
    expect(xdebugApply(w).attributes("disabled")).toBeDefined();
  });

  it("stages a mode change as unsaved work without calling the daemon", async () => {
    const w = mountPanel();
    await modeBox(w, "profile").setValue(true);

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
    await modeBox(w, "profile").setValue(true);
    await startSelect(w).setValue("trigger");
    await xdebugApply(w).trigger("click");
    await vi.waitFor(() => expect(setXdebug).toHaveBeenCalled());

    expect(setXdebug).toHaveBeenCalledWith("8.3", null, ["debug", "profile"], "trigger");
    expect(w.emitted("updated")?.[0]).toEqual([saved]);
    expect(lastDirty(w)).toEqual([false]);
    expect((modeBox(w, "profile").element as HTMLInputElement).checked).toBe(true);
    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
  });

  it("applies only the modes when only the modes were edited", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await modeBox(w, "profile").setValue(true);
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
    await modeBox(w, "profile").setValue(true);
    await w.setProps({ xdebug: { enabled: true, modes: ["debug"], start: "trigger" } });

    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
    expect((modeBox(w, "profile").element as HTMLInputElement).checked).toBe(true);

    await modeBox(w, "profile").setValue(false);
    expect(lastDirty(w)).toEqual([false]);
    expect((startSelect(w).element as HTMLSelectElement).value).toBe("trigger");
  });

  it("shows the latest saved modes when an edit is reverted after a refresh", async () => {
    const w = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    await modeBox(w, "profile").setValue(true);
    await w.setProps({ xdebug: { enabled: true, modes: ["debug", "trace"], start: "always" } });
    await modeBox(w, "profile").setValue(false);

    expect((modeBox(w, "trace").element as HTMLInputElement).checked).toBe(true);
    expect((modeBox(w, "profile").element as HTMLInputElement).checked).toBe(false);
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
    await modeBox(w, "profile").setValue(true);
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
      await modeBox(w, "profile").setValue(true);
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
      await modeBox(w, "profile").setValue(true);
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
      await modeBox(w, "profile").setValue(true);
      await xdebugApply(w).trigger("click");
      await flushPromises();
      await vi.advanceTimersByTimeAsync(10_000);
      await flushPromises();

      const saved = {
        ...xdebugOn,
        xdebug: { "8.3": { enabled: true, modes: ["debug", "profile"], start: "always" } },
      };
      setXdebug.mockResolvedValueOnce(saved);
      await xdebugApply(w).trigger("click");
      await flushPromises();
      finishRead(xdebugOn);
      await flushPromises();

      expect(w.emitted("updated")).toEqual([[saved]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("explains the managed directives while Xdebug is on", () => {
    expect(mountPanel().find('[data-testid="xdebug-directives-hint"]').exists()).toBe(false);
    const on = mountPanel({ xdebug: xdebugOn.xdebug["8.3"] });
    expect(on.find('[data-testid="xdebug-directives-hint"]').text()).toContain("opcache.jit");
  });

  it("won't let the last checked mode be unchecked", async () => {
    const w = mountPanel();
    const debug = modeBox(w, "debug");
    expect(debug.attributes("disabled")).toBeDefined();

    await modeBox(w, "coverage").setValue(true);
    expect(debug.attributes("disabled")).toBeUndefined();
    await debug.setValue(false);
    expect(modeBox(w, "coverage").attributes("disabled")).toBeDefined();
    expect((modeBox(w, "coverage").element as HTMLInputElement).checked).toBe(true);
  });

  it("discards staged Xdebug edits with the rest of the panel", async () => {
    const w = mountPanel();
    await modeBox(w, "trace").setValue(true);
    await byText(w, "Discard")!.trigger("click");

    expect((modeBox(w, "trace").element as HTMLInputElement).checked).toBe(false);
    expect(lastDirty(w)).toEqual([false]);
  });

  it("shows not available and disables every control when no build exists", () => {
    const w = mountPanel({ xdebugAvailable: false });
    expect(w.find('[data-testid="xdebug-unavailable"]').text()).toContain(
      "Xdebug isn't published for this PHP version on this platform.",
    );
    expect(xdebugSwitch(w).attributes("disabled")).toBeDefined();
    expect(modeBox(w, "develop").attributes("disabled")).toBeDefined();
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
    expect(hint.text()).toContain("port 9003 by default");
    expect(hint.text()).toContain("xdebug.client_port");
    await hint.find("button").trigger("click");
    expect(openInBrowser).toHaveBeenCalledWith("https://yerd.app/guide/xdebug");
  });

  it("is hidden for a version that is no longer installed", () => {
    const w = mountPanel({ installedVersion: false, extensions: [XDEBUG] });
    expect(w.find('[data-testid="xdebug-section"]').exists()).toBe(false);
  });
});
