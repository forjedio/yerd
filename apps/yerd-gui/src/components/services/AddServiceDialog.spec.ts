import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  addableServiceTypes: vi.fn(),
  listSites: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({ IpcError: class IpcError extends Error {}, ...client }));

import AddServiceDialog from "./AddServiceDialog.vue";
import type { AddableServiceType, ServiceStatus, SiteEntry } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

function type(overrides: Partial<AddableServiceType>): AddableServiceType {
  return {
    type_id: "mysql",
    display_name: "MySQL",
    multiplicity: "single",
    requires_site: false,
    requires_version: true,
    already_installed: false,
    available_versions: ["8.4.0", "9.7.1"],
    default_port: 3306,
    suggested_port: 3306,
    ...overrides,
  };
}

const TYPES = [
  type({ already_installed: true }),
  type({
    type_id: "meilisearch",
    display_name: "Meilisearch",
    available_versions: ["1.48.0", "1.49.0"],
    default_port: 7700,
    suggested_port: 7701,
  }),
  type({
    type_id: "reverb",
    display_name: "Reverb",
    multiplicity: "per_site",
    requires_site: true,
    requires_version: false,
    available_versions: [],
    default_port: 8080,
    suggested_port: 8083,
  }),
];

const S3_TYPE = type({
  type_id: "versitygw",
  display_name: "S3 (versitygw)",
  available_versions: ["1.7.0"],
  default_port: 7070,
  suggested_port: 7070,
});

function site(name: string, laravel = true): SiteEntry {
  return { name, document_root: `/srv/${name}`, php: "8.4", secure: false, kind: "linked", is_laravel: laravel };
}

function reverbInstance(siteName: string): ServiceStatus {
  return {
    service: `reverb:${siteName}`,
    type_id: "reverb",
    display_name: "Reverb",
    installed_versions: [],
    selected_version: null,
    state: "running",
    pid: 1,
    listen: null,
    port: 8081,
    enabled: false,
    supports_databases: false,
    site: siteName,
  };
}

async function mountDialog(props: Record<string, unknown> = {}) {
  const w = mount(AddServiceDialog, {
    attachTo: document.body,
    props: { open: true, adding: false, services: [], tld: "test", ...props },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

type Dialog = Awaited<ReturnType<typeof mountDialog>>;

async function pick(w: Dialog, id: string) {
  await w.get(`[data-type="${id}"]`).trigger("click");
  await flushPromises();
}

function submit(w: Dialog) {
  return w.get('[data-testid="add-submit"]');
}

beforeEach(() => {
  client.addableServiceTypes.mockReset().mockResolvedValue(TYPES);
  client.listSites.mockReset().mockResolvedValue([site("codestash"), site("vito4"), site("plain", false)]);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("AddServiceDialog", () => {
  it("shows a tile per type in daemon order with installed ones disabled", async () => {
    const w = await mountDialog();
    const tiles = w.findAll("[data-type]");
    expect(tiles.map((t) => t.attributes("data-type"))).toEqual(["mysql", "meilisearch", "reverb"]);
    expect(tiles[0]!.text()).toContain("Added");
    expect(tiles[0]!.attributes("disabled")).toBeDefined();
    expect(tiles[2]!.text()).toContain("One per Laravel site");
    expect(w.text()).toContain("Pick a service to add.");
    expect(submit(w).attributes("disabled")).toBeDefined();
  });

  it("preselects the latest version and the suggested port, marked next free until edited", async () => {
    const w = await mountDialog();
    await pick(w, "meilisearch");
    expect((w.get('select[aria-label="Version"]').element as HTMLSelectElement).value).toBe("1.49.0");
    expect((w.get('input[aria-label="Port"]').element as HTMLInputElement).value).toBe("7701");
    expect(w.find('[data-testid="next-free"]').exists()).toBe(true);
    expect(w.get('[aria-label="Start with Yerd"]').attributes("aria-checked")).toBe("true");

    await w.get('input[aria-label="Port"]').setValue("7800");
    expect(w.find('[data-testid="next-free"]').exists()).toBe(false);
  });

  it("offers Laravel sites without a Reverb as domains and names the site's PHP", async () => {
    const w = await mountDialog({ services: [reverbInstance("codestash")] });
    await pick(w, "reverb");
    const options = w.get('select[aria-label="Laravel site"]').findAll("option");
    expect(options.map((o) => o.text())).toEqual(["vito4.test"]);
    expect(w.get('[data-testid="add-subtitle"]').text()).toBe("Runs on vito4.test's PHP — nothing to download");
    expect(w.get('[aria-label="Start with Yerd"]').attributes("aria-checked")).toBe("false");
  });

  it("explains a missing Laravel site and keeps Add disabled", async () => {
    client.listSites.mockResolvedValue([site("plain", false)]);
    const w = await mountDialog();
    await pick(w, "reverb");
    expect(w.text()).toContain("No Laravel sites found.");
    expect(submit(w).attributes("disabled")).toBeDefined();
  });

  it("disables Add for an invalid port", async () => {
    const w = await mountDialog();
    await pick(w, "meilisearch");
    await w.get('input[aria-label="Port"]').setValue("70000");
    expect(submit(w).attributes("disabled")).toBeDefined();
  });

  it("emits the exact add arguments for each kind", async () => {
    const w = await mountDialog();
    await pick(w, "meilisearch");
    expect(submit(w).text()).toBe("Add Meilisearch");
    await submit(w).trigger("click");
    expect(w.emitted("add")?.[0]).toEqual([
      { type_id: "meilisearch", site: null, port: 7701, version: "1.49.0", autostart: true },
      "Meilisearch",
    ]);

    await pick(w, "reverb");
    await submit(w).trigger("click");
    expect(w.emitted("add")?.[1]).toEqual([
      { type_id: "reverb", site: "codestash", port: 8083, version: null, autostart: false },
      "Reverb",
    ]);
  });

  it("describes S3 storage: tile note, both autostart subtitles and the submit label", async () => {
    client.addableServiceTypes.mockResolvedValue([...TYPES, S3_TYPE]);
    const w = await mountDialog();
    expect(w.get('[data-type="versitygw"]').text()).toContain("Object storage");
    await pick(w, "versitygw");
    expect(w.get('[data-testid="add-subtitle"]').text()).toBe(
      "Downloads versitygw 1.7.0, generates access keys for this install, and starts it now and whenever Yerd starts.",
    );
    await w.get('[aria-label="Start with Yerd"]').trigger("click");
    expect(w.get('[data-testid="add-subtitle"]').text()).toBe(
      "Downloads versitygw 1.7.0, generates access keys for this install, and starts it now.",
    );
    expect(submit(w).text()).toBe("Add S3 storage");
    await submit(w).trigger("click");
    expect(w.emitted("add")?.[0]).toEqual([
      { type_id: "versitygw", site: null, port: 7070, version: "1.7.0", autostart: false },
      "S3 (versitygw)",
    ]);
  });

  it("locks every control and ignores dismissal while adding", async () => {
    const w = await mountDialog();
    await pick(w, "meilisearch");
    await w.setProps({ adding: true });
    await flushPromises();
    const dialog = w.get('[role="dialog"]');
    const controls = dialog.findAll("input, select, button").filter((c) => c.attributes("aria-label") !== "Close");
    for (const control of controls) expect(control.attributes("disabled")).toBeDefined();
    expect(submit(w).text()).toBe("Installing…");
    expect(dialog.text()).toContain("this can take a few minutes");

    await dialog.get('[aria-label="Close"]').trigger("click");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flushPromises();
    expect(w.emitted("update:open")).toBeUndefined();
  });
});
