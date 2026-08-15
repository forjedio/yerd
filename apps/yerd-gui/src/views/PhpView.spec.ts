import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

// The runtime column header and the pool/process wording follow the host OS,
// so the platform singleton is mocked with a settable value.
const hostPlatform = vi.hoisted(() => ({ value: "linux" }));
vi.mock("@/composables/usePlatform", async () => {
  const { computed, ref } = await import("vue");
  const platform = ref(hostPlatform.value);
  return {
    loadPlatform: () => Promise.resolve(),
    usePlatform: () => {
      platform.value = hostPlatform.value;
      return {
        platform,
        isMac: computed(() => platform.value === "macos"),
        isLinux: computed(() => platform.value === "linux"),
        isWindows: computed(() => platform.value === "windows"),
        supportsPathInstall: computed(() => true),
      };
    },
  };
});

import PhpView from "./PhpView.vue";
import { useDaemon } from "@/composables/useDaemon";
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
}) {
  const installed = opts.installed ?? ["8.1", "8.4"];
  const available = opts.available ?? ["8.5"];
  const legacy = opts.legacy ?? ["7.4", "8.0"];
  const extensions = opts.extensions ?? {};
  const pool = opts.pool ?? {};
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_php":
        return Promise.resolve({
          type: "php_versions",
          installed,
          default: "8.4",
          updates: [],
          settings: {},
          version_settings: {},
          pool,
        });
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

  it("has no per-version card when nothing is installed or registered", async () => {
    stubIpc({ installed: [] });
    const wrapper = await mountView();

    expect(wrapper.text()).not.toContain("Per-version configuration");
  });
});

// Windows serves through php-cgi and has no FPM pool, so a screen labelled
// "FPM" there names something the machine does not run.
describe("PhpView runtime vocabulary", () => {
  afterEach(() => {
    hostPlatform.value = "linux";
  });

  it("labels the runtime column and copy for the host OS", async () => {
    stubIpc({});
    const unix = await mountView();
    expect(unix.findAll("th").map((t) => t.text())).toContain("FPM");
    expect(unix.text()).toContain("FPM pools");

    hostPlatform.value = "windows";
    stubIpc({});
    const win = await mountView();
    expect(win.findAll("th").map((t) => t.text())).toContain("php-cgi");
    expect(win.text()).toContain("FastCGI processes");
    expect(win.text()).not.toContain("FPM");
  });
});
