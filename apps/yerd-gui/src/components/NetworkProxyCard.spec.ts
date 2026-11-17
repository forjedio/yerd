import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const getNetworkSettings = vi.fn();
const setNetworkSettings = vi.fn();
const testNetwork = vi.fn();
vi.mock("@/ipc/client", () => ({
  getNetworkSettings: (...a: unknown[]) => getNetworkSettings(...a),
  setNetworkSettings: (...a: unknown[]) => setNetworkSettings(...a),
  testNetwork: (...a: unknown[]) => testNetwork(...a),
  IpcError: class IpcError extends Error {},
}));

import NetworkProxyCard from "./NetworkProxyCard.vue";
import type { NetworkSettings } from "@/ipc/types";

function systemSettings(): NetworkSettings {
  return {
    mode: "system",
    no_proxy: [],
    effective: {
      mode: "system",
      source: "macos_system_settings",
      http_proxy: "http://u:***@proxy.corp:8080",
      https_proxy: "http://u:***@proxy.corp:8080",
      bypass: ["localhost", ".test"],
    },
  };
}

function manualSettings(): NetworkSettings {
  return {
    mode: "manual",
    http_proxy: "http://u:***@proxy.corp:3128",
    no_proxy: [".corp.com"],
    effective: {
      mode: "manual",
      source: "manual",
      http_proxy: "http://u:***@proxy.corp:3128",
      https_proxy: "http://u:***@proxy.corp:3128",
      bypass: ["localhost", ".test", ".corp.com"],
    },
  };
}

async function mountCard() {
  const wrapper = mount(NetworkProxyCard);
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  getNetworkSettings.mockReset().mockResolvedValue(systemSettings());
  setNetworkSettings.mockReset().mockImplementation(async () => manualSettings());
  testNetwork.mockReset();
});

describe("NetworkProxyCard", () => {
  it("shows the detected proxy and its source", async () => {
    const wrapper = await mountCard();
    expect(wrapper.get('[data-testid="network-detected"]').text()).toContain(
      "http://u:***@proxy.corp:8080 (macOS System Settings)",
    );
    expect(wrapper.find('[data-testid="network-http"]').exists()).toBe(false);
  });

  it("reports no proxy when nothing was detected", async () => {
    getNetworkSettings.mockResolvedValue({
      mode: "system",
      no_proxy: [],
      effective: { mode: "system", source: "none", bypass: [] },
    });
    const wrapper = await mountCard();
    expect(wrapper.get('[data-testid="network-detected"]').text()).toContain("No proxy detected");
  });

  it("saves a manual proxy, sending an empty HTTPS field as an empty string", async () => {
    const wrapper = await mountCard();
    await wrapper.get('select[aria-label="Proxy mode"]').setValue("manual");
    await wrapper.get('[data-testid="network-http"]').setValue("http://u:pw@proxy.corp:3128");
    await wrapper.get('[data-testid="network-bypass"]').setValue("corp.com, 10.0.0.0/8 ,");
    await wrapper.get('[data-testid="network-save"]').trigger("click");
    await flushPromises();
    expect(setNetworkSettings).toHaveBeenCalledWith(
      "manual",
      "http://u:pw@proxy.corp:3128",
      "",
      ["corp.com", "10.0.0.0/8"],
    );
    expect(wrapper.html()).not.toContain("pw@");
  });

  it("keeps stored URLs when switching to system or off", async () => {
    getNetworkSettings.mockResolvedValue(manualSettings());
    const wrapper = await mountCard();
    expect(
      (wrapper.get('[data-testid="network-http"]').element as HTMLInputElement).value,
    ).toBe("http://u:***@proxy.corp:3128");
    for (const mode of ["off", "system"] as const) {
      setNetworkSettings.mockClear();
      await wrapper.get('select[aria-label="Proxy mode"]').setValue(mode);
      await wrapper.get('[data-testid="network-save"]').trigger("click");
      await flushPromises();
      expect(setNetworkSettings).toHaveBeenCalledWith(mode, undefined, undefined, [".corp.com"]);
      getNetworkSettings.mockResolvedValue(manualSettings());
    }
  });

  it("disables Save after a failed load and retries on demand", async () => {
    getNetworkSettings.mockRejectedValueOnce(new Error("daemon unreachable"));
    const wrapper = await mountCard();
    expect(wrapper.get('[data-testid="network-save"]').attributes("disabled")).toBeDefined();
    expect(wrapper.get('[data-testid="network-error"]').text()).toContain("daemon unreachable");
    await wrapper.get('[data-testid="network-save"]').trigger("click");
    expect(setNetworkSettings).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="network-retry"]').trigger("click");
    await flushPromises();
    expect(getNetworkSettings).toHaveBeenCalledTimes(2);
    expect(wrapper.find('[data-testid="network-retry"]').exists()).toBe(false);
    expect(wrapper.get('[data-testid="network-save"]').attributes("disabled")).toBeUndefined();
  });

  it("shows a save error inline", async () => {
    setNetworkSettings.mockRejectedValue(new Error("HTTP proxy: invalid proxy URL: re-enter the proxy password"));
    const wrapper = await mountCard();
    await wrapper.get('[data-testid="network-save"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="network-error"]').text()).toContain("re-enter the proxy password");
  });

  it("disables Test connection until unsaved changes are saved", async () => {
    const wrapper = await mountCard();
    const test = () => wrapper.get('[data-testid="network-test"]');
    expect(test().attributes("disabled")).toBeUndefined();
    expect(wrapper.find('[data-testid="network-dirty"]').exists()).toBe(false);
    await wrapper.get('[data-testid="network-bypass"]').setValue("corp.com");
    expect(test().attributes("disabled")).toBeDefined();
    expect(wrapper.get('[data-testid="network-dirty"]').text()).toContain(
      "Save to test these settings",
    );
    await test().trigger("click");
    expect(testNetwork).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="network-save"]').trigger("click");
    await flushPromises();
    expect(test().attributes("disabled")).toBeUndefined();
  });

  it("renders a test result with the route it took", async () => {
    testNetwork.mockResolvedValue({
      ok: true,
      url: "https://files.yerd.app/latest.json",
      status: 200,
      via: "http://u:***@proxy.corp:8080",
      source: "login_shell",
    });
    const wrapper = await mountCard();
    await wrapper.get('[data-testid="network-test"]').trigger("click");
    await flushPromises();
    const text = wrapper.get('[data-testid="network-result"]').text();
    expect(text).toContain("Connected");
    expect(text).toContain("HTTP 200");
    expect(text).toContain("via http://u:***@proxy.corp:8080 (login shell)");
  });

  it("renders a failed test", async () => {
    testNetwork.mockResolvedValue({
      ok: false,
      url: "https://files.yerd.app/latest.json",
      error: "connection refused",
      source: "none",
    });
    const wrapper = await mountCard();
    await wrapper.get('[data-testid="network-test"]').trigger("click");
    await flushPromises();
    const text = wrapper.get('[data-testid="network-result"]').text();
    expect(text).toContain("Failed");
    expect(text).toContain("direct: connection refused");
  });
});
