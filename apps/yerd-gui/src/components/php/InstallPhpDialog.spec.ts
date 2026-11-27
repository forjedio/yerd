import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const availablePhp = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp,
}));

import InstallPhpDialog from "./InstallPhpDialog.vue";
import { useToast } from "@/composables/useToast";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

function stubAvailable(available: string[], legacy: string[], installed: string[]) {
  availablePhp.mockResolvedValue({ type: "available_php", available, legacy, installed });
}

async function mountDialog(props: Record<string, unknown> = {}) {
  const w = mount(InstallPhpDialog, {
    attachTo: document.body,
    props: { open: true, installing: false, ...props },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

type Dialog = Awaited<ReturnType<typeof mountDialog>>;

function row(w: Dialog, version: string) {
  return w.get(`[data-version="${version}"]`);
}

async function pick(w: Dialog, version: string) {
  await row(w, version).trigger("click");
  await flushPromises();
}

function submit(w: Dialog) {
  return w.get('[data-testid="install-submit"]');
}

async function acknowledge(w: Dialog) {
  await w.get('[data-testid="legacy-ack"]').setValue(true);
}

beforeEach(() => {
  availablePhp.mockReset();
  stubAvailable(["8.4", "8.5"], ["7.4", "8.0"], ["8.4"]);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("InstallPhpDialog", () => {
  it("lists both groups newest first with installed rows disabled", async () => {
    const w = await mountDialog();
    const versions = (group: string) =>
      w
        .get(`[data-group="${group}"]`)
        .findAll("[data-version]")
        .map((r) => r.attributes("data-version"));
    expect(versions("supported")).toEqual(["8.5", "8.4"]);
    expect(versions("legacy")).toEqual(["8.0", "7.4"]);
    expect(row(w, "8.4").attributes("disabled")).toBeDefined();
    expect(row(w, "8.4").text()).toContain("Installed");
    expect(row(w, "8.5").text()).toContain("Available");
  });

  it("installs the preselected supported version without the legacy flag", async () => {
    const w = await mountDialog();
    expect(submit(w).text()).toBe("Install 8.5");
    expect(w.find('[data-testid="legacy-warning"]').exists()).toBe(false);
    await submit(w).trigger("click");
    expect(w.emitted("install")?.[0]).toEqual([{ version: "8.5", legacy: false }]);
    const opens = w.emitted("update:open");
    expect(opens?.[opens.length - 1]).toEqual([false]);
  });

  it("gates a legacy pick behind the acknowledgement", async () => {
    const w = await mountDialog();
    await pick(w, "8.0");
    expect(w.get('[data-testid="legacy-warning"]').text()).toContain("PHP 8.0 is out of support");
    expect(submit(w).text()).toBe("Install 8.0");
    expect(submit(w).attributes("disabled")).toBeDefined();

    await acknowledge(w);
    expect(submit(w).attributes("disabled")).toBeUndefined();
    await submit(w).trigger("click");
    expect(w.emitted("install")?.[0]).toEqual([{ version: "8.0", legacy: true }]);
  });

  it("re-arms the acknowledgement whenever the pick changes", async () => {
    const w = await mountDialog();
    await pick(w, "8.0");
    await acknowledge(w);
    await pick(w, "8.5");
    expect(w.find('[data-testid="legacy-warning"]').exists()).toBe(false);
    await pick(w, "7.4");
    expect(submit(w).attributes("disabled")).toBeDefined();
    expect((w.get('[data-testid="legacy-ack"]').element as HTMLInputElement).checked).toBe(false);
  });

  it("preselects the newest legacy version with the warning when no supported one is left", async () => {
    stubAvailable(["8.5"], ["7.4", "8.0"], ["8.5"]);
    const w = await mountDialog();
    expect(submit(w).text()).toBe("Install 8.0");
    expect(w.find('[data-testid="legacy-warning"]').exists()).toBe(true);
    await acknowledge(w);
    await submit(w).trigger("click");
    expect(w.emitted("install")?.[0]).toEqual([{ version: "8.0", legacy: true }]);
  });

  it("says so when nothing is left to install", async () => {
    stubAvailable(["8.5"], [], ["8.5"]);
    const w = await mountDialog();
    expect(w.find('[data-testid="install-empty"]').exists()).toBe(true);
    expect(submit(w).attributes("disabled")).toBeDefined();
  });

  it("is disabled while another install runs", async () => {
    const w = await mountDialog({ installing: true });
    expect(submit(w).attributes("disabled")).toBeDefined();
    await submit(w).trigger("click");
    expect(w.emitted("install")).toBeUndefined();
  });

  it("toasts when the list can't be loaded", async () => {
    availablePhp.mockRejectedValue(new Error("offline"));
    await mountDialog();
    expect(useToast().toasts.value.map((t) => t.title)).toContain(
      "Couldn't load installable versions",
    );
  });
});
