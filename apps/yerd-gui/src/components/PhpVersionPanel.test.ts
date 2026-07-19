import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import PhpVersionPanel from "./PhpVersionPanel.vue";
import type { PhpExtInfo } from "@/ipc/types";

const setPhpDirectives = vi.hoisted(() => vi.fn());
const removePhpExtension = vi.hoisted(() => vi.fn());

// Vitest module mocks are total, so every client symbol the panel imports has
// to appear here or it arrives as undefined.
vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  removePhpExtension,
  setPhpDirectives,
  setPhpVersionSettings: vi.fn(),
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
      extensions: [],
      installedVersion: true,
      extensionsLoading: false,
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
