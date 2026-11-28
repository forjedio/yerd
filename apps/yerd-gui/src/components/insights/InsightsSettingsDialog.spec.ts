import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DumpsStatusResponse, InsightsThresholds } from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  dumpsStatus: vi.fn(),
  setDumpsEnabled: vi.fn(),
  setDumpFeature: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  dumpsStatus: mocks.dumpsStatus,
  setDumpsEnabled: mocks.setDumpsEnabled,
  setDumpFeature: mocks.setDumpFeature,
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: vi.fn(), error: mocks.toastError }),
}));

import InsightsSettingsDialog from "./InsightsSettingsDialog.vue";

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

const THRESHOLDS: InsightsThresholds = {
  n_plus_one_min: 5,
  duplicate_min: 2,
  slow_query_ms: 100,
  large_read_rows: 1000,
  max_queries: 50,
};

function status(overrides: Partial<DumpsStatusResponse> = {}): DumpsStatusResponse {
  return {
    type: "dumps_status",
    enabled: true,
    port: 2304,
    running: true,
    persist: false,
    extensions: [
      { version: "8.4", present: true },
      { version: "8.3", present: false },
      { version: "7.4", present: false, legacy: true },
    ],
    counts: COUNTS,
    features: { http: false },
    ...overrides,
  };
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountDialog(thresholds: InsightsThresholds | null = THRESHOLDS) {
  const wrapper = mount(InsightsSettingsDialog, {
    attachTo: document.body,
    props: { open: true, thresholds, saving: false },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function el<T extends Element>(selector: string): T {
  const found = document.body.querySelector<T>(selector);
  if (!found) throw new Error(`no ${selector}`);
  return found;
}

function input(label: string): HTMLInputElement {
  return el<HTMLInputElement>(`input[aria-label="${label}"]`);
}

async function type(label: string, value: string): Promise<void> {
  const field = input(label);
  field.value = value;
  field.dispatchEvent(new Event("input"));
  await flushPromises();
}

function chips(): string[] {
  return Array.from(document.body.querySelectorAll('[data-testid="ext-chip"]')).map((c) => c.textContent?.trim() ?? "");
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.dumpsStatus.mockResolvedValue(status());
  mocks.setDumpsEnabled.mockResolvedValue(undefined);
  mocks.setDumpFeature.mockResolvedValue(undefined);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("InsightsSettingsDialog", () => {
  it("fills the five thresholds from the current values", async () => {
    await mountDialog();
    expect(
      ["N+1 queries", "Duplicate queries", "Slow query", "Large read", "Too many queries"].map((l) => input(l).value),
    ).toEqual(["5", "2", "100", "1000", "50"]);
  });

  it("spells the current value into each threshold hint", async () => {
    await mountDialog();
    expect(el('[data-threshold="slow_query_ms"]').textContent).toContain("A single query slower than 100 ms");
    await type("N+1 queries", "8");
    expect(el('[data-threshold="n_plus_one_min"]').textContent).toContain("at least 8 times");
  });

  it("saves the edited thresholds", async () => {
    const w = await mountDialog();
    await type("N+1 queries", "8");
    el<HTMLButtonElement>('[data-testid="save-thresholds"]').click();
    await flushPromises();
    expect(w.emitted("save")?.[0]).toEqual([{ ...THRESHOLDS, n_plus_one_min: 8 }]);
  });

  it("blocks a duplicate threshold below 2 and a zero threshold", async () => {
    const w = await mountDialog();
    await type("Duplicate queries", "1");
    await type("Slow query", "0");
    const save = el<HTMLButtonElement>('[data-testid="save-thresholds"]');
    expect(save.disabled).toBe(true);
    save.click();
    await flushPromises();
    expect(w.emitted("save")).toBeUndefined();
    expect(document.body.querySelectorAll('[data-testid="threshold-error"]')).toHaveLength(2);
  });

  it("labels each PHP version's extension, legacy as timing only", async () => {
    await mountDialog();
    expect(chips()).toEqual(["INSTALLED", "NOT INSTALLED", "TOO OLD: TIMING ONLY"]);
    expect(el('[data-testid="dump-server-address"]').textContent).toContain("127.0.0.1:2304");
  });

  it("applies capture and signal switches through the client", async () => {
    await mountDialog();
    el<HTMLButtonElement>('[aria-label="Outgoing HTTP (off by default)"]').click();
    await flushPromises();
    expect(mocks.setDumpFeature).toHaveBeenCalledWith("http", true);
    el<HTMLButtonElement>('[aria-label="Capture from the PHP extension"]').click();
    await flushPromises();
    expect(mocks.setDumpsEnabled).toHaveBeenCalledWith(false);
    expect(mocks.dumpsStatus).toHaveBeenCalledTimes(3);
  });

  it("disables the signals while capture is off", async () => {
    mocks.dumpsStatus.mockResolvedValue(status({ enabled: false }));
    await mountDialog();
    expect(el<HTMLButtonElement>('[aria-label="Cache events"]').disabled).toBe(true);
  });

  it("emits changePort from the dump server row", async () => {
    const w = await mountDialog();
    const link = Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Change port");
    link?.click();
    expect(w.emitted("changePort")).toHaveLength(1);
  });
});
