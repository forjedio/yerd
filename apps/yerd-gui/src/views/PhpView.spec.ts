import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

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
}) {
  const installed = opts.installed ?? ["8.1", "8.4"];
  const available = opts.available ?? ["8.5"];
  const legacy = opts.legacy ?? ["7.4", "8.0"];
  const extensions = opts.extensions ?? {};
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

    // The legacy disclosure is present; reveal it.
    const toggle = wrapper.find('[data-testid="toggle-legacy"]');
    expect(toggle.exists()).toBe(true);
    await toggle.trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="legacy-warning"]').exists()).toBe(true);

    // The footer "Install legacy version" button is disabled until confirmed.
    const installBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install legacy version"));
    expect(installBtn).toBeTruthy();
    expect(installBtn!.attributes("disabled")).toBeDefined();

    // Tick the confirmation switch → enabled, and installing streams the flag.
    await wrapper.find('button[aria-label="Confirm legacy install"]').trigger("click");
    await flushPromises();
    const enabled = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install legacy version"));
    expect(enabled!.attributes("disabled")).toBeUndefined();

    await enabled!.trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ confirmLegacy: true });
  });

  it("keeps the stable install available while the legacy disclosure is open", async () => {
    stubIpc({});
    const wrapper = await mountView();

    const openBtn = wrapper
      .findAll("button")
      .find((b) => b.text().includes("Install") && b.attributes("disabled") === undefined);
    await openBtn!.trigger("click");
    await flushPromises();

    await wrapper.find('[data-testid="toggle-legacy"]').trigger("click");
    await flushPromises();

    const stableBtn = wrapper.find('[data-testid="install-stable"]');
    expect(stableBtn.exists()).toBe(true);
    expect(stableBtn.attributes("disabled")).toBeUndefined();

    await stableBtn.trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ confirmLegacy: false });
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
