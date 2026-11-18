import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import PhpView from "./PhpView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { resetResourceCache } from "@/composables/useResource";
import type { PhpVersion } from "@/ipc/types";

/** A default mock: installed 8.4 + legacy 8.1, an available list with a legacy
 *  8.0, `ok` for mutations, and a loud reject for anything unexpected. */
function stubIpc(opts: {
  installed?: PhpVersion[];
  available?: PhpVersion[];
  legacy?: PhpVersion[];
  extensions?: Record<string, unknown[]>;
  pool?: Record<string, Record<string, string>>;
  settings?: Record<string, string>;
  ffiVersions?: PhpVersion[];
  libvipsFound?: boolean | null;
  xdebug?: Record<string, unknown>;
  xdebugVersions?: PhpVersion[];
}) {
  const installed = opts.installed ?? ["8.1", "8.4"];
  const available = opts.available ?? ["8.5"];
  const legacy = opts.legacy ?? ["7.4", "8.0"];
  const extensions = opts.extensions ?? {};
  const pool = opts.pool ?? {};
  const settings = opts.settings ?? {};
  const phpVersions = () => ({
    type: "php_versions",
    installed,
    default: "8.4",
    updates: [],
    settings,
    version_settings: {},
    pool,
    ffi_versions: opts.ffiVersions,
    libvips_found: opts.libvipsFound,
    xdebug: opts.xdebug,
    xdebug_versions: opts.xdebugVersions,
  });
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_php":
        return Promise.resolve(phpVersions());
      case "set_php_settings":
        return Promise.resolve({
          type: "php_versions",
          installed,
          default: "8.4",
          settings: {},
        });
      case "check_php_updates":
        return Promise.resolve(phpVersions());
      case "list_php_extensions":
        return Promise.resolve({ type: "php_extensions", by_version: extensions });
      case "available_php":
        return Promise.resolve({ type: "available_php", available, installed, legacy });
      case "install_php_streamed":
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: { unmount: () => void }[] = [];

async function mountView() {
  const wrapper = mount(PhpView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

describe("PhpView legacy handling", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("tags an installed legacy version with a legacy badge", async () => {
    stubIpc({ installed: ["8.1", "8.4"] });
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("legacy");
  });

  it("gates the legacy install behind a confirmation checkbox", async () => {
    stubIpc({});
    const wrapper = await mountView();

    // Open the install modal.
    const openBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install") && b.attributes("disabled") === undefined);
    expect(openBtn).toBeTruthy();
    await openBtn!.trigger("click");
    await flushPromises();

    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(false);
    const toggle = wrapper.find('[data-testid="toggle-legacy"]');
    expect(toggle.exists()).toBe(true);
    await toggle.trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(true);

    const installBtn = wrapper.find('[data-testid="install-submit"]');
    expect(installBtn.attributes("disabled")).toBeDefined();

    await wrapper.find('button[aria-label="Confirm legacy install"]').trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="install-submit"]').attributes("disabled")).toBeUndefined();

    await wrapper.find('[data-testid="install-submit"]').trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ version: "8.0", confirmLegacy: true });
  });

  it("installs the stable version while the legacy toggle is off", async () => {
    stubIpc({});
    const wrapper = await mountView();

    const openBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install") && b.attributes("disabled") === undefined);
    await openBtn!.trigger("click");
    await flushPromises();

    const stableBtn = wrapper.find('[data-testid="install-submit"]');
    expect(stableBtn.exists()).toBe(true);
    expect(stableBtn.attributes("disabled")).toBeUndefined();

    await stableBtn.trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ confirmLegacy: false });
  });

  it("re-arms the opt-in when the legacy toggle is switched back off", async () => {
    stubIpc({});
    const wrapper = await mountView();

    const openBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install") && b.attributes("disabled") === undefined);
    await openBtn!.trigger("click");
    await flushPromises();

    await wrapper.find('[data-testid="toggle-legacy-label"]').trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(true);

    await wrapper.find('button[aria-label="Confirm legacy install"]').trigger("click");
    await flushPromises();

    await wrapper.find('[data-testid="toggle-legacy"]').trigger("click");
    await wrapper.find('[data-testid="toggle-legacy"]').trigger("click");
    await flushPromises();

    expect(wrapper.find('[data-testid="install-submit"]').attributes("disabled")).toBeDefined();
  });

  it("starts in legacy mode, locked, when no stable version is left to install", async () => {
    stubIpc({ available: [] });
    const wrapper = await mountView();

    const openBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install") && b.attributes("disabled") === undefined);
    await openBtn!.trigger("click");
    await flushPromises();

    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="toggle-legacy"]').attributes("disabled")).toBeDefined();

    await wrapper.find('[data-testid="toggle-legacy-label"]').trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(true);

    await wrapper.find('button[aria-label="Confirm legacy install"]').trigger("click");
    await flushPromises();
    await wrapper.find('[data-testid="install-submit"]').trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ version: "8.0", confirmLegacy: true });
  });
});

describe("PhpView per-version configuration", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  function tabLabels(wrapper: Awaited<ReturnType<typeof mountView>>) {
    return wrapper.findAll('[role="tab"]').map((t) => t.text());
  }

  it("gives every installed version a tab and starts on the default", async () => {
    stubIpc({ installed: ["8.1", "8.4"] });
    const wrapper = await mountView();

    expect(tabLabels(wrapper)).toHaveLength(2);
    const selected = wrapper
      .findAll('[role="tab"]')
      .find((t) => t.attributes("aria-selected") === "true");
    expect(selected!.text()).toContain("8.4");
  });

  it("counts a pool override in the version's tab badge and passes it to the panel", async () => {
    stubIpc({ installed: ["8.1", "8.4"], pool: { "8.4": { max_children: "32" } } });
    const wrapper = await mountView();

    const tab = wrapper
      .findAll('[role="tab"]')
      .find((t) => t.text().includes("8.4"));
    expect(tab!.text()).toContain("1");
    expect(
      (wrapper.find('input[id="pool-8.4-max-children"]').element as HTMLInputElement)
        .value,
    ).toBe("32");
    expect(
      (wrapper.find('input[id="pool-8.1-max-children"]').element as HTMLInputElement)
        .value,
    ).toBe("");
  });

  it("keeps hidden panels mounted so unsaved edits survive a tab switch", async () => {
    stubIpc({ installed: ["8.1", "8.4"] });
    const wrapper = await mountView();

    expect(wrapper.find('input[id="set-8.1-memory_limit"]').exists()).toBe(true);
    expect(wrapper.find('input[id="set-8.4-memory_limit"]').exists()).toBe(true);
  });

  it("shows only the active version's panel", async () => {
    stubIpc({ installed: ["8.1", "8.4"] });
    const wrapper = await mountView();

    const panels = wrapper.findAll('[role="tabpanel"]');
    expect(panels).toHaveLength(2);
    // Newest first, and 8.4 is the default, so 8.4's panel leads and shows.
    expect(panels[0].attributes("hidden")).toBeUndefined();
    expect(panels[1].attributes("hidden")).toBeDefined();
  });

  it("lists versions newest first", async () => {
    stubIpc({ installed: ["8.1", "8.4"] });
    const wrapper = await mountView();

    expect(tabLabels(wrapper).map((t) => t.trim())).toEqual(["8.4", "8.1"]);
  });

  it("surfaces an uninstalled version that still has registered extensions", async () => {
    stubIpc({
      installed: ["8.4"],
      extensions: {
        "8.2": [{ name: "xdebug", path: "/tmp/xdebug.so", zend: true, present: false }],
      },
    });
    const wrapper = await mountView();

    expect(tabLabels(wrapper).some((t) => t.includes("8.2"))).toBe(true);
    expect(wrapper.text()).toContain("not installed");
  });

  it("badges only the versions with Xdebug on and passes availability to the panel", async () => {
    stubIpc({
      installed: ["8.1", "8.3", "8.4"],
      xdebug: {
        "8.3": { enabled: false, modes: ["debug"], start: "trigger" },
        "8.4": { enabled: true, modes: ["debug"], start: "always" },
      },
      xdebugVersions: ["8.3", "8.4"],
    });
    const wrapper = await mountView();

    const rows = wrapper.findAll("tbody tr");
    const rowFor = (v: string) => rows.find((r) => r.text().includes(`PHP ${v}`));
    expect(rowFor("8.4")!.find('[data-testid="xdebug-badge"]').exists()).toBe(true);
    expect(rowFor("8.3")!.find('[data-testid="xdebug-badge"]').exists()).toBe(false);
    expect(rowFor("8.1")!.find('[data-testid="xdebug-badge"]').exists()).toBe(false);
    expect(wrapper.findAll('[data-testid="xdebug-badge"]')).toHaveLength(1);

    expect(
      wrapper.find('button[aria-label="Xdebug for PHP 8.4"]').attributes("aria-checked"),
    ).toBe("true");
    expect(wrapper.findAll('[data-testid="xdebug-unavailable"]')).toHaveLength(1);

    const shownPanelFor = async (v: string) => {
      const tab = wrapper.findAll('[role="tab"]').find((t) => t.text().includes(v));
      await tab!.trigger("mousedown", { button: 0 });
      await flushPromises();
      const shown = wrapper
        .findAll('[role="tabpanel"]')
        .filter((p) => p.attributes("hidden") === undefined);
      expect(shown).toHaveLength(1);
      return shown[0];
    };

    const old = await shownPanelFor("8.1");
    expect(old.find('[data-testid="xdebug-unavailable"]').exists()).toBe(true);
    expect(
      old.find('button[aria-label="Xdebug for PHP 8.1"]').attributes("disabled"),
    ).toBeDefined();

    const current = await shownPanelFor("8.3");
    expect(current.find('[data-testid="xdebug-unavailable"]').exists()).toBe(false);
    const sw = current.find('button[aria-label="Xdebug for PHP 8.3"]');
    expect(sw.attributes("disabled")).toBeUndefined();
    expect(sw.attributes("aria-checked")).toBe("false");
  });

  it("applies the Xdebug state from the update check response without a second list call", async () => {
    stubIpc({ installed: ["8.4"], xdebugVersions: ["8.4"] });
    let finishCheck: (r: unknown) => void = () => {};
    const wrapper = await mountView();
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) => {
      if (cmd === "check_php_updates") {
        return new Promise((resolve) => {
          finishCheck = resolve;
        });
      }
      return base(cmd, ...rest);
    });
    const listCalls = () => invokeMock.mock.calls.filter(([c]) => c === "list_php").length;
    const before = listCalls();
    expect(wrapper.find('[data-testid="xdebug-badge"]').exists()).toBe(false);

    const refreshBtn = wrapper.findAll("button").find((b) => b.text().trim() === "Refresh");
    await refreshBtn!.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("check_php_updates", undefined);
    expect(refreshBtn!.attributes("disabled")).toBeDefined();

    finishCheck({
      type: "php_versions",
      installed: ["8.4"],
      default: "8.4",
      updates: [],
      settings: {},
      version_settings: {},
      pool: {},
      xdebug: { "8.4": { enabled: true, modes: ["debug"], start: "always" } },
      xdebug_versions: ["8.4"],
    });
    await flushPromises();

    expect(listCalls()).toBe(before);
    expect(refreshBtn!.attributes("disabled")).toBeUndefined();
    expect(wrapper.find('[data-testid="xdebug-badge"]').exists()).toBe(true);
    expect(
      wrapper.find('button[aria-label="Xdebug for PHP 8.4"]').attributes("aria-checked"),
    ).toBe("true");
    expect(useToast().toasts.value.map((t) => t.title)).toContain("Update check complete");
  });

  it("has no per-version card when nothing is installed or registered", async () => {
    stubIpc({ installed: [] });
    const wrapper = await mountView();

    expect(wrapper.text()).not.toContain("Per-version configuration");
  });
});

describe("PhpView FFI switch", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  const toggle = '[data-testid="toggle-ffi"]';

  function lastSettingsPayload(): Record<string, string> | undefined {
    const calls = invokeMock.mock.calls.filter(([cmd]) => cmd === "set_php_settings");
    const last = calls[calls.length - 1];
    return (last?.[1] as { settings: Record<string, string> } | undefined)?.settings;
  }

  async function save(wrapper: Awaited<ReturnType<typeof mountView>>) {
    const button = wrapper.findAll("button").find((b) => b.text() === "Save");
    await button!.trigger("click");
    await flushPromises();
  }

  it("is disabled with a note when no installed version has FFI", async () => {
    stubIpc({ installed: ["8.4"], ffiVersions: [] });
    const wrapper = await mountView();
    expect(wrapper.find(toggle).attributes("disabled")).toBeDefined();
    expect(wrapper.find('[data-testid="ffi-unavailable"]').exists()).toBe(true);
  });

  it("stays enabled when already on so a CLI-set value can be cleared", async () => {
    stubIpc({ installed: ["8.4"], ffiVersions: [], settings: { "ffi.enable": "On" } });
    const wrapper = await mountView();
    expect(wrapper.find(toggle).attributes("disabled")).toBeUndefined();
    await wrapper.find(toggle).trigger("click");
    await save(wrapper);
    expect(lastSettingsPayload()?.["ffi.enable"]).toBe("");
  });

  it("keeps a saved On switch usable after toggling it off unsaved", async () => {
    stubIpc({ installed: ["8.4"], ffiVersions: [], settings: { "ffi.enable": "On" } });
    const wrapper = await mountView();
    await wrapper.find(toggle).trigger("click");
    expect(wrapper.find(toggle).attributes("aria-checked")).toBe("false");
    expect(wrapper.find(toggle).attributes("disabled")).toBeUndefined();
    await wrapper.find(toggle).trigger("click");
    expect(wrapper.find(toggle).attributes("aria-checked")).toBe("true");
  });

  it("sends On when switched on and saved", async () => {
    stubIpc({ installed: ["8.4"], ffiVersions: ["8.4"] });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="ffi-unavailable"]').exists()).toBe(false);
    await wrapper.find(toggle).trigger("click");
    await save(wrapper);
    expect(lastSettingsPayload()?.["ffi.enable"]).toBe("On");
  });

  it("shows the libvips hint only while on and libvips is missing", async () => {
    stubIpc({ installed: ["8.4"], ffiVersions: ["8.4"], libvipsFound: false });
    const wrapper = await mountView();
    const hint = '[data-testid="ffi-global"] [data-testid="libvips-hint"]';
    expect(wrapper.find(hint).exists()).toBe(false);
    await wrapper.find(toggle).trigger("click");
    expect(wrapper.find(hint).exists()).toBe(true);
  });

  it("makes no FFI claim before the PHP list has loaded", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "list_php":
          return Promise.reject(new Error("daemon unavailable"));
        case "list_php_extensions":
          return Promise.resolve({ type: "php_extensions", by_version: {} });
        default:
          return Promise.reject(new Error(`unexpected invoke ${cmd}`));
      }
    });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="ffi-unavailable"]').exists()).toBe(false);
    expect(wrapper.find(toggle).exists()).toBe(true);
    expect(wrapper.find(toggle).attributes("disabled")).toBeUndefined();
  });
});
