import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import ServicesView from "./ServicesView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { resetResourceCache } from "@/composables/useResource";
import type { AddableServiceType } from "@/ipc/types";

function meilisearchType(): AddableServiceType {
  return {
    type_id: "meilisearch",
    display_name: "Meilisearch",
    multiplicity: "single",
    requires_site: false,
    requires_version: true,
    already_installed: false,
    available_versions: ["1.49.0"],
    default_port: 7700,
    suggested_port: 7700,
  };
}

/** No services installed yet and Meilisearch offered in the Add dialog - the
 *  state a first-time install starts from. `add_service` is left pending so the
 *  in-flight window can be asserted. */
function stubIpc(): { resolveAdd: () => void } {
  let resolveAdd = (): void => {};
  const pending = new Promise<{ type: string; id: string }>((resolve) => {
    resolveAdd = () => resolve({ type: "service_instance_id", id: "meilisearch" });
  });
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_services":
        return Promise.resolve({ type: "services", services: [] });
      case "addable_service_types":
        return Promise.resolve({ type: "addable_services", types: [meilisearchType()] });
      case "list_sites":
        return Promise.resolve({ type: "sites", sites: [] });
      case "list_databases":
        return Promise.resolve({ type: "databases", databases: [] });
      case "add_service":
        return pending;
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
  return { resolveAdd };
}

const mounted: { unmount: () => void }[] = [];

async function mountView() {
  const wrapper = mount(ServicesView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

/** Exact-match, so the dialog's "Add" is not shadowed by the header's
 *  "Add Service". */
function findButton(wrapper: Awaited<ReturnType<typeof mountView>>, text: string) {
  const btn = wrapper.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button with text "${text}"`);
  return btn;
}

/** Walk the Add dialog to step 2, leaving it ready to submit. Nothing is
 *  pre-selected, so the type has to be picked before Continue enables. */
async function openAddToConfigureStep(wrapper: Awaited<ReturnType<typeof mountView>>) {
  await findButton(wrapper, "Add Service").trigger("click");
  await flushPromises();
  await wrapper.find('select[aria-label="Service type"]').setValue("meilisearch");
  await flushPromises();
  await findButton(wrapper, "Continue").trigger("click");
  await flushPromises();
}

describe("ServicesView add-in-flight state", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("swaps the Add button for a disabled spinner while the install runs", async () => {
    const { resolveAdd } = stubIpc();
    const wrapper = await mountView();
    await openAddToConfigureStep(wrapper);

    expect(findButton(wrapper, "Add").attributes("disabled")).toBeUndefined();

    await findButton(wrapper, "Add").trigger("click");
    await flushPromises();

    const submit = findButton(wrapper, "Installing…");
    expect(submit.attributes("disabled")).toBeDefined();
    expect(submit.find(".animate-spin").exists()).toBe(true);

    resolveAdd();
    await flushPromises();
  });

  it("keeps the dialog open and locks every control until the install finishes", async () => {
    const { resolveAdd } = stubIpc();
    const wrapper = await mountView();
    await openAddToConfigureStep(wrapper);
    await findButton(wrapper, "Add").trigger("click");
    await flushPromises();

    const dialog = wrapper.find('[role="dialog"]');
    expect(dialog.exists()).toBe(true);
    expect(dialog.text()).toContain("this can take a few minutes");
    for (const control of dialog.findAll("input, select, button")) {
      expect(control.attributes("disabled")).toBeDefined();
    }
    expect(dialog.find('[aria-label="Close"]').exists()).toBe(false);

    resolveAdd();
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });

  it("closes the dialog and clears the busy state when the install fails", async () => {
    stubIpc();
    invokeMock.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "list_services":
          return Promise.resolve({ type: "services", services: [] });
        case "addable_service_types":
          return Promise.resolve({ type: "addable_services", types: [meilisearchType()] });
        case "list_sites":
          return Promise.resolve({ type: "sites", sites: [] });
        case "list_databases":
          return Promise.resolve({ type: "databases", databases: [] });
        case "add_service":
          return Promise.reject(new Error("download failed"));
        default:
          return Promise.reject(new Error(`unexpected invoke ${cmd}`));
      }
    });
    const wrapper = await mountView();
    await openAddToConfigureStep(wrapper);
    await findButton(wrapper, "Add").trigger("click");
    await flushPromises();

    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });
});
