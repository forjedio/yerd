import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const serviceOverrides = vi.fn();
const setServiceOverride = vi.fn();
const unsetServiceOverride = vi.fn();
vi.mock("@/ipc/client", () => ({
  serviceOverrides: (...a: unknown[]) => serviceOverrides(...a),
  setServiceOverride: (...a: unknown[]) => setServiceOverride(...a),
  unsetServiceOverride: (...a: unknown[]) => unsetServiceOverride(...a),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import ServiceOverridesModal from "./ServiceOverridesModal.vue";
import type { ServiceStatus } from "@/ipc/types";

function service(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "mysql",
    display_name: "MySQL",
    installed_versions: ["9.7.1"],
    selected_version: "9.7.1",
    state: "running",
    pid: 42,
    listen: "127.0.0.1:3306",
    port: 3306,
    enabled: true,
    supports_databases: true,
    supports_overrides: true,
    ...overrides,
  };
}

async function mountModal(s: ServiceStatus = service()) {
  const wrapper = mount(ServiceOverridesModal, {
    props: { open: true, service: s },
    global: { stubs: { teleport: true } },
  });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  serviceOverrides.mockReset().mockResolvedValue({});
  setServiceOverride.mockReset().mockResolvedValue(undefined);
  unsetServiceOverride.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("ServiceOverridesModal", () => {
  it("lists the fetched overrides in name order", async () => {
    serviceOverrides.mockResolvedValue({
      max_connections: "500",
      innodb_buffer_pool_size: "256M",
    });
    const wrapper = await mountModal();

    expect(serviceOverrides).toHaveBeenCalledWith("mysql");
    const rows = wrapper.findAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("innodb_buffer_pool_size");
    expect(rows[1].text()).toContain("max_connections");
    expect(rows[1].text()).toContain("500");
  });

  it("shows an empty state when the service has no overrides", async () => {
    const wrapper = await mountModal();
    expect(wrapper.text()).toContain("No overrides for this service.");
  });

  it("adds an override, toasts the restart hint, and refetches", async () => {
    const wrapper = await mountModal();
    await wrapper.find('[aria-label="Override name"]').setValue("max_connections");
    await wrapper.find('[aria-label="Override value"]').setValue("500");
    serviceOverrides.mockResolvedValue({ max_connections: "500" });

    await wrapper.findAll("button").filter((b) => b.text().includes("Add"))[0].trigger("click");
    await flushPromises();

    expect(setServiceOverride).toHaveBeenCalledWith("mysql", "max_connections", "500");
    expect(serviceOverrides).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalledWith("Saved - restart MySQL to apply");
    expect(wrapper.findAll("li")).toHaveLength(1);
  });

  it("removes an override by its name", async () => {
    serviceOverrides.mockResolvedValue({ max_connections: "500" });
    const wrapper = await mountModal();

    await wrapper.find('[aria-label="Remove override max_connections"]').trigger("click");
    await flushPromises();

    expect(unsetServiceOverride).toHaveBeenCalledWith("mysql", "max_connections");
    expect(toastSuccess).toHaveBeenCalledWith("Saved - restart MySQL to apply");
  });

  it("surfaces a daemon rejection verbatim as an error toast", async () => {
    setServiceOverride.mockRejectedValue(
      new Error("bind-address is managed by Yerd: Yerd pins the engine to loopback"),
    );
    const wrapper = await mountModal();
    await wrapper.find('[aria-label="Override name"]').setValue("bind-address");
    await wrapper.find('[aria-label="Override value"]').setValue("0.0.0.0");

    await wrapper.findAll("button").filter((b) => b.text().includes("Add"))[0].trigger("click");
    await flushPromises();

    expect(toastError).toHaveBeenCalledWith(
      "Override change failed",
      "bind-address is managed by Yerd: Yerd pins the engine to loopback",
    );
  });

  it("refetches when the view points it at another service", async () => {
    const wrapper = await mountModal();
    expect(serviceOverrides).toHaveBeenCalledTimes(1);

    await wrapper.setProps({ service: service({ service: "redis", display_name: "Valkey" }) });
    await flushPromises();

    expect(serviceOverrides).toHaveBeenCalledTimes(2);
    expect(serviceOverrides).toHaveBeenLastCalledWith("redis");
  });
});
