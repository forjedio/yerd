import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

import ServiceOverridesCard from "./ServiceOverridesCard.vue";
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

async function mountCard(s: ServiceStatus = service()) {
  const wrapper = mount(ServiceOverridesCard, { props: { service: s }, attachTo: document.body });
  await flushPromises();
  return wrapper;
}

type Card = Awaited<ReturnType<typeof mountCard>>;

function button(wrapper: Card, text: string) {
  const btn = wrapper.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

async function openAddRow(wrapper: Card) {
  await button(wrapper, "Add override").trigger("click");
}

afterEach(() => {
  document.body.innerHTML = "";
});

beforeEach(() => {
  serviceOverrides.mockReset().mockResolvedValue({});
  setServiceOverride.mockReset().mockResolvedValue(undefined);
  unsetServiceOverride.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("ServiceOverridesCard", () => {
  it("lists the fetched overrides in name order", async () => {
    serviceOverrides.mockResolvedValue({
      max_connections: "500",
      innodb_buffer_pool_size: "256M",
    });
    const wrapper = await mountCard();

    expect(serviceOverrides).toHaveBeenCalledWith("mysql");
    const rows = wrapper.findAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0]!.text()).toContain("innodb_buffer_pool_size");
    expect(rows[1]!.text()).toContain("max_connections");
    expect(rows[1]!.text()).toContain("500");
    expect(wrapper.text()).toContain("Written into MySQL's config on every start.");
  });

  it("shows an empty state when the service has no overrides", async () => {
    const wrapper = await mountCard();
    expect(wrapper.text()).toContain("No overrides yet.");
  });

  it("adds an override, toasts the restart hint, refetches and hides the row", async () => {
    const wrapper = await mountCard();
    await openAddRow(wrapper);
    await wrapper.find('[aria-label="Override name"]').setValue("max_connections");
    await wrapper.find('[aria-label="Override value"]').setValue("500");
    serviceOverrides.mockResolvedValue({ max_connections: "500" });

    await button(wrapper, "Add").trigger("click");
    await flushPromises();

    expect(setServiceOverride).toHaveBeenCalledWith("mysql", "max_connections", "500");
    expect(serviceOverrides).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalledWith("Saved - restart MySQL to apply");
    expect(wrapper.findAll("li")).toHaveLength(1);
    expect(wrapper.find('[aria-label="Override name"]').exists()).toBe(false);
  });

  it("removes an override by its name", async () => {
    serviceOverrides.mockResolvedValue({ max_connections: "500" });
    const wrapper = await mountCard();

    await wrapper.find('[aria-label="Remove override max_connections"]').trigger("click");
    await flushPromises();

    expect(unsetServiceOverride).toHaveBeenCalledWith("mysql", "max_connections");
    expect(toastSuccess).toHaveBeenCalledWith("Saved - restart MySQL to apply");
  });

  it("surfaces a daemon rejection verbatim as an error toast and keeps the row", async () => {
    setServiceOverride.mockRejectedValue(
      new Error("bind-address is managed by Yerd: Yerd pins the engine to loopback"),
    );
    const wrapper = await mountCard();
    await openAddRow(wrapper);
    await wrapper.find('[aria-label="Override name"]').setValue("bind-address");
    await wrapper.find('[aria-label="Override value"]').setValue("0.0.0.0");

    await button(wrapper, "Add").trigger("click");
    await flushPromises();

    expect(toastError).toHaveBeenCalledWith(
      "Override change failed",
      "bind-address is managed by Yerd: Yerd pins the engine to loopback",
    );
    expect(wrapper.find('[aria-label="Override name"]').exists()).toBe(true);
  });

  it("refetches when the inspector points it at another service", async () => {
    const wrapper = await mountCard();
    expect(serviceOverrides).toHaveBeenCalledTimes(1);

    await wrapper.setProps({ service: service({ service: "redis", display_name: "Valkey" }) });
    await flushPromises();

    expect(serviceOverrides).toHaveBeenCalledTimes(2);
    expect(serviceOverrides).toHaveBeenLastCalledWith("redis");
  });

  it("hides the add row on Cancel without a call", async () => {
    const wrapper = await mountCard();
    await openAddRow(wrapper);
    await wrapper.find('[aria-label="Override name"]').setValue("x");
    await button(wrapper, "Cancel").trigger("click");
    expect(wrapper.find('[aria-label="Override name"]').exists()).toBe(false);
    expect(setServiceOverride).not.toHaveBeenCalled();
  });

  it("cancels on Escape without letting it reach the drawer", async () => {
    const wrapper = await mountCard();
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    await openAddRow(wrapper);
    await wrapper.find('[aria-label="Override name"]').trigger("keydown", { key: "Escape" });
    document.removeEventListener("keydown", outer);
    expect(wrapper.find('[aria-label="Override name"]').exists()).toBe(false);
    expect(outer).not.toHaveBeenCalled();
  });
});
