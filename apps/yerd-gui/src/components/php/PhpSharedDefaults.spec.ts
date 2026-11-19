import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import PhpSharedDefaults from "./PhpSharedDefaults.vue";
import { listPhp } from "@/ipc/client";
import { resetResourceCache, useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import type { PhpVersionsResponse } from "@/ipc/types";

function data(over: Partial<PhpVersionsResponse> = {}): PhpVersionsResponse {
  return {
    type: "php_versions",
    installed: ["8.4"],
    default: "8.4",
    settings: {},
    ffi_versions: ["8.4"],
    ...over,
  };
}

const saved = data({ settings: { memory_limit: "1G" } });
const mounted: VueWrapper[] = [];

function mountPane(props: Record<string, unknown> = {}) {
  const w = mount(PhpSharedDefaults, {
    attachTo: document.body,
    props: { data: data(), restarting: false, anyRunning: true, ...props },
  });
  mounted.push(w);
  return w;
}

type Pane = ReturnType<typeof mountPane>;

const toggle = '[data-testid="toggle-ffi"]';

function field(w: Pane, key: string) {
  return w.find(`input[id="set-${key}"]`);
}

async function save(w: Pane) {
  await w.get('[data-testid="shared-save"]').trigger("click");
  await flushPromises();
}

function lastPayload(): Record<string, string> | undefined {
  const calls = invokeMock.mock.calls.filter(([cmd]) => cmd === "set_php_settings");
  const last = calls[calls.length - 1];
  return (last?.[1] as { settings: Record<string, string> } | undefined)?.settings;
}

function lastDirty(w: Pane) {
  const events = w.emitted("dirty");
  return events?.[events.length - 1];
}

function segment(w: Pane, label: string) {
  return w
    .get('[aria-label="Display errors"]')
    .findAll('[role="radio"]')
    .find((b) => b.text() === label);
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation((cmd: string) =>
    cmd === "set_php_settings" ? Promise.resolve(saved) : Promise.reject(new Error(`unexpected ${cmd}`)),
  );
  resetResourceCache();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("PhpSharedDefaults FFI switch", () => {
  it("is disabled with a note when no installed version has FFI", () => {
    const w = mountPane({ data: data({ ffi_versions: [] }) });
    expect(w.get(toggle).attributes("disabled")).toBeDefined();
    expect(w.find('[data-testid="ffi-unavailable"]').exists()).toBe(true);
  });

  it("stays enabled when already on so a CLI-set value can be cleared", async () => {
    const w = mountPane({ data: data({ ffi_versions: [], settings: { "ffi.enable": "On" } }) });
    expect(w.get(toggle).attributes("disabled")).toBeUndefined();
    await w.get(toggle).trigger("click");
    await save(w);
    expect(lastPayload()?.["ffi.enable"]).toBe("");
  });

  it("keeps a saved On switch usable after toggling it off unsaved", async () => {
    const w = mountPane({ data: data({ ffi_versions: [], settings: { "ffi.enable": "On" } }) });
    await w.get(toggle).trigger("click");
    expect(w.get(toggle).attributes("aria-checked")).toBe("false");
    expect(w.get(toggle).attributes("disabled")).toBeUndefined();
    await w.get(toggle).trigger("click");
    expect(w.get(toggle).attributes("aria-checked")).toBe("true");
  });

  it("sends On when switched on and saved", async () => {
    const w = mountPane();
    expect(w.find('[data-testid="ffi-unavailable"]').exists()).toBe(false);
    await w.get(toggle).trigger("click");
    await save(w);
    expect(lastPayload()?.["ffi.enable"]).toBe("On");
  });

  it("shows the libvips hint only while on and libvips is missing", async () => {
    const w = mountPane({ data: data({ libvips_found: false }) });
    const hint = '[data-testid="ffi-global"] [data-testid="libvips-hint"]';
    expect(w.find(hint).exists()).toBe(false);
    await w.get(toggle).trigger("click");
    expect(w.find(hint).exists()).toBe(true);
  });

  it("makes no FFI claim before the PHP list has loaded", () => {
    const w = mountPane({ data: null });
    expect(w.find('[data-testid="ffi-unavailable"]').exists()).toBe(false);
    expect(w.get(toggle).attributes("disabled")).toBeUndefined();
  });
});

describe("PhpSharedDefaults form", () => {
  it("seeds from the shared settings", () => {
    const w = mountPane({ data: data({ settings: { memory_limit: "512M" } }) });
    expect((field(w, "memory_limit").element as HTMLInputElement).value).toBe("512M");
  });

  it("keeps unsaved edits when the data changes and reseeds when pristine", async () => {
    const w = mountPane();
    await field(w, "memory_limit").setValue("2G");
    await w.setProps({ data: data({ settings: { memory_limit: "512M" } }) });
    expect((field(w, "memory_limit").element as HTMLInputElement).value).toBe("2G");

    const clean = mountPane();
    await clean.setProps({ data: data({ settings: { memory_limit: "512M" } }) });
    expect((field(clean, "memory_limit").element as HTMLInputElement).value).toBe("512M");
  });

  it("sends every key on save, writes the shared cache and emits the reply", async () => {
    const cache = useResource("php", listPhp, { immediate: false });
    const w = mountPane();
    await field(w, "memory_limit").setValue("1G");
    await save(w);

    const payload = lastPayload()!;
    expect(payload.memory_limit).toBe("1G");
    expect(payload.max_input_time).toBe("");
    expect(Object.keys(payload).sort()).toEqual(
      [
        "display_errors",
        "error_reporting",
        "ffi.enable",
        "max_execution_time",
        "max_file_uploads",
        "max_input_time",
        "memory_limit",
        "post_max_size",
        "upload_max_filesize",
      ].sort(),
    );
    expect(cache.data.value).toEqual(saved);
    expect(w.emitted("updated")?.[0]).toEqual([saved]);
    expect(useToast().toasts.value.map((t) => t.title)).toContain("PHP settings updated");
  });

  it.each([
    ["Default", ""],
    ["On", "On"],
    ["Off", "Off"],
  ])("stages display_errors %s as %j", async (label, expected) => {
    const w = mountPane({ data: data({ settings: { display_errors: label === "Default" ? "On" : "" } }) });
    await segment(w, label)!.trigger("click");
    await save(w);
    expect(lastPayload()?.display_errors).toBe(expected);
  });

  it("reports dirty on an edit and clean after saving", async () => {
    const w = mountPane();
    expect(lastDirty(w)).toEqual([false]);
    await field(w, "memory_limit").setValue("1G");
    expect(lastDirty(w)).toEqual([true]);
    await save(w);
    expect(lastDirty(w)).toEqual([false]);
  });
});

describe("PhpSharedDefaults restart all", () => {
  function restartAll(w: Pane) {
    return w.findAll("button").find((b) => b.text().includes("Restart all pools"))!;
  }

  it("emits a restart request", async () => {
    const w = mountPane();
    await restartAll(w).trigger("click");
    expect(w.emitted("restartAll")).toHaveLength(1);
  });

  it("is disabled when nothing runs", () => {
    const w = mountPane({ anyRunning: false });
    expect(restartAll(w).attributes("disabled")).toBeDefined();
  });
});
