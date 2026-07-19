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
function stubIpc(opts: { installed?: PhpVersion[]; available?: PhpVersion[]; legacy?: PhpVersion[] }) {
  const installed = opts.installed ?? ["8.1", "8.4"];
  const available = opts.available ?? ["8.5"];
  const legacy = opts.legacy ?? ["7.4", "8.0"];
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
        return Promise.resolve({ type: "php_extensions", by_version: {} });
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
