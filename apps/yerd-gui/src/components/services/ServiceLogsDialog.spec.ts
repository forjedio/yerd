import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const serviceLogs = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  serviceLogs,
}));

import ServiceLogsDialog from "./ServiceLogsDialog.vue";
import { useToast } from "@/composables/useToast";
import type { ServiceStatus } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];
const clipboardWriteText = vi.fn();

function postgres(): ServiceStatus {
  return {
    service: "postgres",
    display_name: "PostgreSQL",
    installed_versions: ["18.4"],
    selected_version: "18.4",
    state: "running",
    pid: 1,
    listen: "127.0.0.1:5432",
    port: 5432,
    enabled: true,
    supports_databases: true,
  };
}

async function mountDialog(props: Record<string, unknown> = {}) {
  const w = mount(ServiceLogsDialog, {
    attachTo: document.body,
    props: { open: true, service: postgres(), title: "PostgreSQL logs", ...props },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

function errorToasts(): string[] {
  return useToast()
    .toasts.value.filter((t) => t.title === "Couldn't read logs")
    .map((t) => t.title);
}

beforeEach(() => {
  vi.useFakeTimers();
  serviceLogs.mockReset().mockResolvedValue(["LOG:  ready", "ERROR:  bad thing", "LOG:  last"]);
  clipboardWriteText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("ServiceLogsDialog", () => {
  it("reads 500 lines on open and again every 2s", async () => {
    await mountDialog();
    expect(serviceLogs).toHaveBeenCalledTimes(1);
    expect(serviceLogs).toHaveBeenCalledWith("postgres", 500);
    await vi.advanceTimersByTimeAsync(2000);
    expect(serviceLogs).toHaveBeenCalledTimes(2);
  });

  it("stops polling when closed", async () => {
    const w = await mountDialog();
    await w.setProps({ open: false });
    await vi.advanceTimersByTimeAsync(6000);
    expect(serviceLogs).toHaveBeenCalledTimes(1);
  });

  it("does not restart when the same service object is replaced", async () => {
    const w = await mountDialog();
    await w.setProps({ service: { ...postgres(), pid: 2 } });
    await flushPromises();
    expect(serviceLogs).toHaveBeenCalledTimes(1);
  });

  it("shows the Live chip after a read and paints error lines red", async () => {
    const w = await mountDialog();
    expect(w.find('[data-testid="logs-live"]').exists()).toBe(true);
    const spans = w.get('[data-testid="service-log"]').findAll("span");
    expect(spans.map((s) => s.text())).toEqual(["LOG:  ready", "ERROR:  bad thing", "LOG:  last"]);
    expect(spans[1]!.classes()).toContain("text-destructive");
    expect(spans[0]!.classes()).not.toContain("text-destructive");
    expect(spans[2]!.classes()).toContain("text-foreground");
    expect(w.text()).toContain("Last 500 lines");
  });

  it("toasts once across repeated failures and drops the Live chip", async () => {
    serviceLogs.mockRejectedValue(new Error("no log file"));
    const w = await mountDialog();
    await vi.advanceTimersByTimeAsync(6000);
    expect(serviceLogs.mock.calls.length).toBeGreaterThan(2);
    expect(errorToasts()).toHaveLength(1);
    expect(w.find('[data-testid="logs-live"]').exists()).toBe(false);
  });

  it("copies the joined lines", async () => {
    const w = await mountDialog();
    const copy = w.findAll("button").find((b) => b.text().trim() === "Copy")!;
    await copy.trigger("click");
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenCalledWith("LOG:  ready\nERROR:  bad thing\nLOG:  last");
  });

  it("asks to close from the Close button", async () => {
    const w = await mountDialog();
    const close = w.findAll("button").find((b) => b.text().trim() === "Close")!;
    await close.trigger("click");
    expect(w.emitted("update:open")?.[0]).toEqual([false]);
  });
});
