import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  createDatabase: vi.fn(),
  dropDatabase: vi.fn(),
  backupDatabase: vi.fn(),
  restoreDatabase: vi.fn(),
  pickSaveFile: vi.fn(),
  pickOpenFile: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({ IpcError: class IpcError extends Error {}, ...client }));

import ServiceDatabasesCard from "./ServiceDatabasesCard.vue";
import { useToast } from "@/composables/useToast";
import type { ServiceStatus } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };
const MenuStub = {
  name: "MenuStub",
  props: ["open"],
  emits: ["update:open"],
  template: '<div data-testid="dropdown"><slot /></div>',
};
const ItemStub = {
  props: ["disabled"],
  emits: ["select"],
  template:
    '<div role="menuitem" :data-disabled="disabled ? \'\' : undefined" @click="$emit(\'select\')"><slot /></div>',
};

const mounted: VueWrapper[] = [];

function postgres(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
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
    ...overrides,
  };
}

const NAMES = ["app", "app_test_1", "app_test_2", "blog", "notes"];

async function mountCard(props: Record<string, unknown> = {}) {
  const w = mount(ServiceDatabasesCard, {
    attachTo: document.body,
    props: { service: postgres(), names: NAMES, loading: false, error: null, connectDb: "app", ...props },
    global: {
      stubs: {
        DropdownMenu: MenuStub,
        DropdownMenuTrigger: Pass,
        DropdownMenuContent: Pass,
        DropdownMenuItem: ItemStub,
        DropdownMenuSeparator: { template: "<hr />" },
      },
    },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

type Card = Awaited<ReturnType<typeof mountCard>>;

function rows(w: Card): string[] {
  return w.findAll("[data-db]").map((r) => r.attributes("data-db")!);
}

function button(w: Card, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

async function menu(w: Card, db: string, action: string) {
  await w.get(`[data-db="${db}"] [data-action="${action}"]`).trigger("click");
  await flushPromises();
}

function toasts() {
  return useToast().toasts.value.map((t) => [t.title, t.detail]);
}

beforeEach(() => {
  for (const fn of Object.values(client)) fn.mockReset().mockResolvedValue(undefined);
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("ServiceDatabasesCard", () => {
  it("folds test copies by default and lists everything with the toggle off", async () => {
    const w = await mountCard();
    expect(rows(w)).toEqual(["app", "blog", "notes"]);
    expect(w.get('[data-db="app"]').text()).toContain("+ 2 test");
    expect(w.get('[data-testid="databases-footer"]').text()).toBe("Showing 3 of 3 · 2 test copies hidden");
    expect(w.get('[data-testid="databases-count"]').text()).toBe("5");

    await w.get('[aria-label="Hide test copies"]').trigger("click");
    expect(rows(w)).toEqual(NAMES);
    expect(w.get('[data-testid="databases-footer"]').text()).toBe("Showing 5 of 5");
  });

  it("filters the rows", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Filter databases"]').setValue("BL");
    expect(rows(w)).toEqual(["blog"]);
    expect(w.get('[data-testid="databases-footer"]').text()).toContain("Showing 1 of 3");
    await w.get('[aria-label="Filter databases"]').setValue("zzz");
    expect(w.text()).toContain('No databases match "zzz".');
  });

  it("validates a new name and creates the database", async () => {
    const w = await mountCard();
    await button(w, "New database").trigger("click");
    const input = w.get('[aria-label="New database name"]');
    await input.setValue("1bad");
    expect(button(w, "Create").attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("Use letters, digits, and underscores");

    await input.setValue("my_app");
    expect(button(w, "Create").attributes("disabled")).toBeUndefined();
    await button(w, "Create").trigger("click");
    await flushPromises();
    expect(client.createDatabase).toHaveBeenCalledWith("postgres", "my_app");
    expect(w.emitted("changed")).toHaveLength(1);
    expect(w.find('[aria-label="New database name"]').exists()).toBe(false);
  });

  it("cancels the create row on Escape without letting it through", async () => {
    const w = await mountCard();
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    await button(w, "New database").trigger("click");
    await w.get('[aria-label="New database name"]').trigger("keydown", { key: "Escape" });
    document.removeEventListener("keydown", outer);
    expect(w.find('[aria-label="New database name"]').exists()).toBe(false);
    expect(outer).not.toHaveBeenCalled();
  });

  it("confirms a delete in place", async () => {
    const w = await mountCard();
    await menu(w, "blog", "drop");
    const row = w.get('[data-testid="confirm-drop"]');
    expect(row.text()).toContain("Delete blog? This can't be undone.");
    expect(document.activeElement?.textContent?.trim()).toBe("Cancel");

    await button(w, "Cancel").trigger("click");
    expect(w.find('[data-testid="confirm-drop"]').exists()).toBe(false);
    expect(client.dropDatabase).not.toHaveBeenCalled();

    await menu(w, "blog", "drop");
    await button(w, "Delete").trigger("click");
    await flushPromises();
    expect(client.dropDatabase).toHaveBeenCalledWith("postgres", "blog");
    expect(w.emitted("changed")).toHaveLength(1);
  });

  it("cancels a confirm row on Escape without letting it through", async () => {
    const w = await mountCard();
    await menu(w, "blog", "drop");
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    await w.get('[data-testid="confirm-drop"] button').trigger("keydown", { key: "Escape" });
    document.removeEventListener("keydown", outer);
    expect(w.find('[data-testid="confirm-drop"]').exists()).toBe(false);
    expect(outer).not.toHaveBeenCalled();
  });

  it("restores from a picked file after an amber confirm", async () => {
    const w = await mountCard();
    client.pickOpenFile.mockResolvedValue(null);
    await menu(w, "notes", "restore");
    expect(w.find('[data-testid="confirm-restore"]').exists()).toBe(false);

    client.pickOpenFile.mockResolvedValue("/tmp/dumps/notes-1103.sql");
    await menu(w, "notes", "restore");
    expect(w.get('[data-testid="confirm-restore"]').text()).toContain("Overwrite notes with notes-1103.sql?");
    await button(w, "Restore").trigger("click");
    await flushPromises();
    expect(client.restoreDatabase).toHaveBeenCalledWith("postgres", "notes", "/tmp/dumps/notes-1103.sql");
    expect(w.emitted("changed")).toHaveLength(1);
  });

  it("backs up to the picked path and toasts where it went", async () => {
    const w = await mountCard();
    client.pickSaveFile.mockResolvedValue("/tmp/app.sql");
    await menu(w, "app", "backup");
    expect(client.pickSaveFile).toHaveBeenCalledWith("app.sql");
    expect(client.backupDatabase).toHaveBeenCalledWith("postgres", "app", "/tmp/app.sql");
    expect(toasts()).toContainEqual(["Backed up app", "Saved to /tmp/app.sql"]);
  });

  it("emits Use in Connect and reports an open row menu as a layer", async () => {
    const w = await mountCard();
    await menu(w, "blog", "use");
    expect(w.emitted("useInConnect")?.[0]).toEqual(["blog"]);

    w.findAllComponents({ name: "MenuStub" })[0]!.vm.$emit("update:open", true);
    await flushPromises();
    const updates = w.emitted("update:layerOpen") ?? [];
    expect(updates[updates.length - 1]).toEqual([true]);
  });

  it("asks to start a stopped engine and disables New database", async () => {
    const w = await mountCard({ service: postgres({ state: "stopped" }), names: null });
    expect(w.text()).toContain("Start PostgreSQL to see its databases.");
    expect(button(w, "New database").attributes("disabled")).toBeDefined();
  });
});
