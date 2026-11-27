import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

import ServicesView from "./ServicesView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { resetResourceCache, useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { listServices } from "@/ipc/client";
import type { AddableServiceType, ServiceStatus, StatusReport } from "@/ipc/types";
import { servicesIntent } from "@/lib/shortcuts/servicesIntent";
import { getViewActions } from "@/lib/shortcuts/useViewActions";

function svc(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "mysql",
    type_id: "mysql",
    display_name: "MySQL",
    installed_versions: ["9.7.1"],
    selected_version: "9.7.1",
    state: "running",
    pid: 42,
    listen: "127.0.0.1:3308",
    port: 3308,
    enabled: true,
    supports_databases: true,
    ...overrides,
  };
}

const MYSQL = svc();
const POSTGRES = svc({
  service: "postgres",
  type_id: "postgres",
  display_name: "PostgreSQL",
  installed_versions: ["18.4"],
  selected_version: "18.4",
  port: 5432,
  pid: 56020,
});
const MARIADB = svc({
  service: "mariadb",
  type_id: "mariadb",
  display_name: "MariaDB",
  installed_versions: ["11.8.8"],
  selected_version: "11.8.8",
  state: "stopped",
  pid: null,
  port: 3306,
});
const REDIS = svc({
  service: "redis",
  type_id: "redis",
  display_name: "Redis (Valkey)",
  installed_versions: ["9.1.0"],
  selected_version: "9.1.0",
  supports_databases: false,
  port: 6379,
});
const REVERB = svc({
  service: "reverb:codestash",
  type_id: "reverb",
  display_name: "Reverb",
  installed_versions: [],
  selected_version: null,
  supports_databases: false,
  site: "codestash",
  port: 8081,
  enabled: false,
});

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

interface StubOpts {
  services?: ServiceStatus[];
  databases?: Record<string, string[]>;
  addService?: () => Promise<unknown>;
  listServices?: () => Promise<unknown>;
  available?: { service: string; available: string[]; installed: string[] }[];
  /** Replaces the reply to one command, e.g. a pending `stop_service`. */
  commands?: Record<string, (args?: Record<string, unknown>) => Promise<unknown>>;
}

function stubIpc(opts: StubOpts = {}) {
  const databases = opts.databases ?? { mysql: ["app", "blog"], postgres: ["main"] };
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    const override = opts.commands?.[cmd];
    if (override) return override(args);
    switch (cmd) {
      case "list_services":
        return opts.listServices?.() ?? Promise.resolve({ type: "services", services: opts.services ?? [MYSQL, POSTGRES, MARIADB, REDIS, REVERB] });
      case "list_databases":
        return Promise.resolve({
          type: "databases",
          databases: (databases[String(args?.service)] ?? []).map((name) => ({ name })),
        });
      case "list_sites":
        return Promise.resolve({
          type: "sites",
          sites: [{ name: "codestash", document_root: "/c", php: "8.4", secure: true, kind: "linked", is_laravel: true }],
        });
      case "list_parked":
        return Promise.resolve({ type: "parked", paths: [] });
      case "list_groups":
        return Promise.resolve({ type: "groups", order: [], members: {} });
      case "addable_service_types":
        return Promise.resolve({ type: "addable_services", types: [meilisearchType()] });
      case "add_service":
        return opts.addService?.() ?? Promise.resolve({ type: "service_instance_id", id: "meilisearch" });
      case "available_services":
        return Promise.resolve({
          type: "available_services",
          services: opts.available ?? [{ service: "mysql", available: ["8.4.0", "9.7.1", "9.8.0"], installed: ["9.7.1"] }],
        });
      case "service_overrides":
        return Promise.resolve({ type: "service_overrides", overrides: {} });
      case "service_logs":
        return Promise.resolve({ type: "service_logs", lines: ["ready"] });
      case "status":
        return Promise.resolve({ type: "status", report: useDaemon().report.value });
      case "start_service":
      case "stop_service":
      case "restart_service":
      case "set_service_autostart":
      case "set_service_port":
      case "change_service_version":
      case "uninstall_service":
      case "remove_service":
      case "create_database":
      case "drop_database":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

let mediaListener: ((e: { matches: boolean }) => void) | null = null;

function setWide(wide: boolean | null): void {
  mediaListener = null;
  globalThis.matchMedia = (
    wide === null
      ? undefined
      : vi.fn(() => ({
          matches: wide,
          addEventListener: (_: string, fn: (e: { matches: boolean }) => void) => {
            mediaListener = fn;
          },
          removeEventListener: vi.fn(),
        }))
  ) as unknown as typeof globalThis.matchMedia;
}

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

async function mountView() {
  const wrapper = mount(ServicesView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: true,
        RouterLink: true,
        DialogPortal: Pass,
        AlertDialogPortal: Pass,
        DropdownMenu: MenuStub,
        DropdownMenuTrigger: Pass,
        DropdownMenuContent: Pass,
        DropdownMenuItem: ItemStub,
        DropdownMenuSeparator: { template: "<hr />" },
      },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type View = Awaited<ReturnType<typeof mountView>>;

function button(w: View, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

function row(w: View, id: string) {
  return w.get(`[data-testid="services-list"] [data-key="${id}"]`);
}

async function select(w: View, id: string) {
  await row(w, id).trigger("click");
  await flushPromises();
}

function inspector(w: View) {
  return w.get('[data-testid="service-inspector"]');
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function dbCalls(service: string): number {
  return calls("list_databases").filter((c) => c[1]?.service === service).length;
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

function escape(): void {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
}

beforeEach(() => {
  invokeMock.mockReset();
  resetResourceCache();
  useDaemon().report.value = { tld: "test", http: { bound: 80 }, https: { bound: 443 } } as unknown as StatusReport;
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  setWide(true);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("ServicesView list", () => {
  it("groups the services in order with their status lines", async () => {
    stubIpc();
    const w = await mountView();
    const groups = w.findAll("[data-section]").map((s) => s.attributes("data-section"));
    expect(groups).toEqual(["databases", "cache", "site"]);
    const status = (id: string) => row(w, id).get('[data-testid="row-status"]').text();
    expect(status("mysql")).toBe("running · :3308 · 2 databases");
    expect(status("postgres")).toBe("running · :5432 · 1 database");
    expect(status("mariadb")).toBe("stopped · :3306");
    expect(status("redis")).toBe("running · :6379");
    expect(row(w, "reverb:codestash").text()).toContain("codestash.test");
  });

  it("lists each running SQL engine once on a wide mount and never on the poll", async () => {
    vi.useFakeTimers();
    stubIpc();
    await mountView();
    expect(dbCalls("mysql")).toBe(1);
    expect(dbCalls("postgres")).toBe(1);
    expect(dbCalls("mariadb")).toBe(0);
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(calls("list_services").length).toBeGreaterThan(1);
    expect(dbCalls("mysql")).toBe(1);
  });

  it("re-lists after the card creates a database", async () => {
    stubIpc();
    const w = await mountView();
    await button(w, "New database").trigger("click");
    await w.get('[aria-label="New database name"]').setValue("shop");
    await button(w, "Create").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("create_database", { service: "mysql", name: "shop" });
    expect(dbCalls("mysql")).toBe(2);
  });

  it("re-lists a running SQL engine when it is selected", async () => {
    stubIpc();
    const w = await mountView();
    await select(w, "postgres");
    expect(dbCalls("postgres")).toBe(2);
  });

  it("lists again after a remount over a cached services list", async () => {
    stubIpc();
    const first = await mountView();
    first.unmount();
    mounted.length = 0;
    await mountView();
    expect(dbCalls("mysql")).toBe(2);
  });

  it("re-lists running SQL engines on the refresh shortcut", async () => {
    stubIpc();
    await mountView();
    getViewActions().refresh?.();
    await flushPromises();
    expect(dbCalls("mysql")).toBe(2);
    expect(dbCalls("postgres")).toBe(2);
  });
});

describe("ServicesView layouts", () => {
  it("selects the first service on a wide window and switches on click", async () => {
    stubIpc();
    const w = await mountView();
    expect(row(w, "mysql").attributes("aria-current")).toBe("true");
    expect(inspector(w).text()).toContain("MySQL");
    await select(w, "redis");
    expect(inspector(w).get('[data-testid="service-header"]').text()).toContain("Redis (Valkey)");
  });

  it("opens the drawer from the compact table and closes it", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    expect(w.get('[data-testid="services-list"]').attributes("data-layout")).toBe("table");
    expect(w.find('[data-testid="service-inspector"]').exists()).toBe(false);
    expect(row(w, "mysql").get('[data-testid="cell-details"]').text()).toBe("2 databases");
    expect(row(w, "reverb:codestash").get('[data-testid="cell-details"]').text()).toBe("WebSockets");
    expect(row(w, "reverb:codestash").get('[data-testid="cell-version"]').text()).toBe("—");

    await select(w, "postgres");
    expect(inspector(w).attributes("role")).toBe("dialog");
    expect(inspector(w).text()).toContain("PostgreSQL");
    expect(dbCalls("postgres")).toBe(2);

    await w.get('[aria-label="Close service details"]').trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="service-inspector"]').exists()).toBe(false);

    await select(w, "postgres");
    expect(dbCalls("postgres")).toBe(3);
  });

  it("closes the drawer when the window turns wide", async () => {
    setWide(false);
    stubIpc();
    const w = await mountView();
    await select(w, "redis");
    mediaListener?.({ matches: true });
    await flushPromises();
    expect(inspector(w).attributes("role")).toBeUndefined();
  });
});

describe("ServicesView actions", () => {
  it("stops, starts and restarts with a toast", async () => {
    stubIpc();
    const w = await mountView();
    await inspector(w).get('button[data-action="stop"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("stop_service", { service: "mysql" });
    expect(toastTitles()).toContain("Stopped MySQL");

    await inspector(w).get('button[data-action="restart"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("restart_service", { service: "mysql" });

    await select(w, "mariadb");
    await inspector(w).get('button[data-action="start"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("start_service", { service: "mariadb" });
    expect(toastTitles()).toContain("Started MariaDB");
  });

  it("toggles Start with Yerd", async () => {
    stubIpc();
    const w = await mountView();
    await w.get('[aria-label="Start MySQL with Yerd"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_service_autostart", { service: "mysql", enabled: false });
  });

  it("edits the port and refuses an invalid one", async () => {
    stubIpc();
    const w = await mountView();
    await button(w, "Edit port").trigger("click");
    await flushPromises();
    await w.get('input[aria-label="Port"]').setValue("70000");
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(calls("set_service_port")).toHaveLength(0);
    expect(toastTitles()).toContain("Invalid port");

    await w.get('input[aria-label="Port"]').setValue("3310");
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_service_port", { service: "mysql", port: 3310 });
  });

  it("changes the version to the latest one not installed", async () => {
    stubIpc();
    const w = await mountView();
    await button(w, "Change").trigger("click");
    await flushPromises();
    await button(w, "Switch").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("change_service_version", { service: "mysql", version: "9.8.0" });
  });

  it("preselects nothing and blocks Switch when only older MySQL versions exist", async () => {
    stubIpc({ available: [{ service: "mysql", available: ["8.0.40", "8.4.9", "9.7.1"], installed: ["9.7.1"] }] });
    const w = await mountView();
    await button(w, "Change").trigger("click");
    await flushPromises();
    const picker = w.get('select[aria-label="version"]');
    expect((picker.element as HTMLSelectElement).value).toBe("");
    expect(w.get('[data-testid="version-note"]').text()).toContain("can't move to an older version");
    expect(button(w, "Switch").attributes("disabled")).toBeDefined();

    await picker.setValue("8.4.9");
    await flushPromises();
    expect(w.get('[data-testid="version-note"]').text()).toContain("data written by v9.7.1 can't be read by v8.4.9");
    expect(button(w, "Switch").attributes("disabled")).toBeDefined();
    await button(w, "Switch").trigger("click");
    await flushPromises();
    expect(calls("change_service_version")).toHaveLength(0);
  });

  it("warns that a PostgreSQL major change starts an empty cluster", async () => {
    stubIpc({ available: [{ service: "postgres", available: ["17.6", "18.4", "19.1"], installed: ["18.4"] }] });
    const w = await mountView();
    await select(w, "postgres");
    await button(w, "Change").trigger("click");
    await flushPromises();
    expect((w.get('select[aria-label="version"]').element as HTMLSelectElement).value).toBe("19.1");
    const note = w.get('[data-testid="version-note"]').text();
    expect(note).toContain("PostgreSQL 19 starts with an empty cluster");
    expect(note).toContain("your PostgreSQL 18 databases stay on disk");
    expect(note).not.toContain("Your stored data is kept");
  });

  it("keeps a second uninstall confirm intact when the first finishes", async () => {
    let finish = (): void => {};
    stubIpc({
      commands: {
        uninstall_service: (args) =>
          args?.service === "mysql"
            ? new Promise((resolve) => {
                finish = () => resolve({ type: "ok" });
              })
            : Promise.resolve({ type: "ok" }),
      },
    });
    const w = await mountView();
    await w.get('[data-action="uninstall"]').trigger("click");
    await flushPromises();
    await button(w, "Uninstall").trigger("click");
    await flushPromises();

    await select(w, "redis");
    await w.get('[data-action="uninstall"]').trigger("click");
    await flushPromises();
    finish();
    await flushPromises();
    expect(w.get('[role="alertdialog"]').text()).toContain("Uninstall Redis (Valkey) 9.1.0?");
    await button(w, "Uninstall").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("uninstall_service", { service: "redis", version: "9.1.0", purge: false });
    expect(toastTitles()).toContain("Uninstalled Redis (Valkey) v9.1.0");
  });

  it("uninstalls the version its menu item names", async () => {
    stubIpc({ services: [svc({ installed_versions: ["8.4.0", "9.7.1"], selected_version: "8.4.0" })] });
    const w = await mountView();
    const item = w.get('[data-action="uninstall"]');
    expect(item.text()).toContain("Uninstall MySQL 8.4.0");
    await item.trigger("click");
    await flushPromises();
    expect(w.get('[role="alertdialog"]').text()).toContain("Uninstall MySQL 8.4.0?");
    await button(w, "Uninstall").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("uninstall_service", { service: "mysql", version: "8.4.0", purge: false });
  });

  it("uninstalls with the purge choice, which resets on reopen", async () => {
    stubIpc();
    const w = await mountView();
    await w.get('[data-action="uninstall"]').trigger("click");
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Uninstall MySQL 9.7.1?");
    expect(dialog.text()).toContain("Your 2 databases stay on disk");

    await button(w, "Cancel").trigger("click");
    await flushPromises();
    expect(calls("uninstall_service")).toHaveLength(0);

    await w.get('[data-action="uninstall"]').trigger("click");
    await flushPromises();
    await w.get('[data-testid="uninstall-purge"]').setValue(true);
    await button(w, "Uninstall").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("uninstall_service", { service: "mysql", version: "9.7.1", purge: true });

    await w.get('[data-action="uninstall"]').trigger("click");
    await flushPromises();
    expect((w.get('[data-testid="uninstall-purge"]').element as HTMLInputElement).checked).toBe(false);
  });

  it("removes a per-site instance at once", async () => {
    stubIpc();
    const w = await mountView();
    await select(w, "reverb:codestash");
    await w.get('[data-action="remove"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("remove_service", { service: "reverb:codestash", purge: false });
  });

  it("opens the logs dialog reading 500 lines", async () => {
    stubIpc();
    const w = await mountView();
    await inspector(w).get('button[data-action="logs"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("service_logs", { service: "mysql", lines: 500 });
    expect(w.text()).toContain("MySQL logs");
  });
});

describe("ServicesView config overrides card", () => {
  it.each([
    ["supported", true, true],
    ["unsupported", false, false],
    ["absent from an older daemon", undefined, false],
  ])("is %s", async (_, supports, shown) => {
    stubIpc({ services: [svc({ supports_overrides: supports })] });
    const w = await mountView();
    expect(w.find('[data-testid="overrides-card"]').exists()).toBe(shown);
  });
});

describe("ServicesView add", () => {
  function pendingAdd() {
    let resolveAdd = (): void => {};
    const pending = new Promise((resolve) => {
      resolveAdd = () => resolve({ type: "service_instance_id", id: "meilisearch" });
    });
    return { pending, resolveAdd: () => resolveAdd() };
  }

  async function openAndPick(w: View) {
    await w.get('[data-testid="add-service"]').trigger("click");
    await flushPromises();
    await w.get('[data-type="meilisearch"]').trigger("click");
    await flushPromises();
  }

  it("swaps the header button for a disabled spinner while the install runs", async () => {
    const { pending, resolveAdd } = pendingAdd();
    stubIpc({ addService: () => pending });
    const w = await mountView();
    await openAndPick(w);
    await w.get('[data-testid="add-submit"]').trigger("click");
    await flushPromises();

    const header = w.get('[data-testid="add-service"]');
    expect(header.attributes("disabled")).toBeDefined();
    expect(header.find(".animate-spin").exists()).toBe(true);
    expect(w.get('[data-testid="add-submit"]').text()).toBe("Installing…");

    resolveAdd();
    await flushPromises();
    expect(toastTitles()).toContain("Added Meilisearch");
  });

  it("keeps the dialog open and locked until the install finishes", async () => {
    const { pending, resolveAdd } = pendingAdd();
    stubIpc({ addService: () => pending });
    const w = await mountView();
    await openAndPick(w);
    await w.get('[data-testid="add-submit"]').trigger("click");
    await flushPromises();

    const dialog = w.get('[role="dialog"]');
    expect(dialog.text()).toContain("this can take a few minutes");
    const controls = dialog.findAll("input, select, button").filter((c) => c.attributes("aria-label") !== "Close");
    for (const control of controls) expect(control.attributes("disabled")).toBeDefined();
    await dialog.get('[aria-label="Close"]').trigger("click");
    await flushPromises();
    expect(w.find('[role="dialog"]').exists()).toBe(true);

    resolveAdd();
    await flushPromises();
    expect(w.find('[role="dialog"]').exists()).toBe(false);
  });

  const MEILI_UNINSTALLED = svc({
    service: "meilisearch",
    type_id: "meilisearch",
    display_name: "Meilisearch",
    installed_versions: [],
    selected_version: null,
    state: "stopped",
    pid: null,
    port: 7700,
    supports_databases: false,
  });

  it("closes the dialog and says it couldn't start when a single-instance add saved before failing", async () => {
    let saved = false;
    stubIpc({
      listServices: () =>
        Promise.resolve({
          type: "services",
          services: [
            MYSQL,
            saved
              ? { ...MEILI_UNINSTALLED, installed_versions: ["1.49.0"], selected_version: "1.49.0", state: "failed" }
              : MEILI_UNINSTALLED,
          ],
        }),
      addService: () => {
        saved = true;
        return Promise.reject(new Error("GLIBC_2.38 not found"));
      },
    });
    const w = await mountView();
    await openAndPick(w);
    await w.get('[data-testid="add-submit"]').trigger("click");
    await flushPromises();
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    expect(toastTitles()).toContain("Meilisearch added, but couldn't start");
    expect(toastTitles()).not.toContain("Couldn't add Meilisearch");
  });

  it("keeps the Add dialog locked while another service action finishes", async () => {
    const { pending, resolveAdd } = pendingAdd();
    let finishStop = (): void => {};
    stubIpc({
      addService: () => pending,
      commands: {
        stop_service: () =>
          new Promise((resolve) => {
            finishStop = () => resolve({ type: "ok" });
          }),
      },
    });
    const w = await mountView();
    await inspector(w).get('button[data-action="stop"]').trigger("click");
    await flushPromises();
    await openAndPick(w);
    await w.get('[data-testid="add-submit"]').trigger("click");
    await flushPromises();

    finishStop();
    await flushPromises();
    expect(toastTitles()).toContain("Stopped MySQL");
    expect(w.get('[data-testid="add-service"]').attributes("disabled")).toBeDefined();
    expect(w.get('[data-testid="add-submit"]').text()).toBe("Installing…");

    resolveAdd();
    await flushPromises();
    expect(w.get('[data-testid="add-service"]').attributes("disabled")).toBeUndefined();
  });

  it("keeps the dialog open and says nothing was added when the add fails before saving", async () => {
    stubIpc({
      services: [MYSQL, MEILI_UNINSTALLED],
      addService: () => Promise.reject(new Error("download failed")),
    });
    const w = await mountView();
    await openAndPick(w);
    await w.get('[data-testid="add-submit"]').trigger("click");
    await flushPromises();
    expect(w.find('[role="dialog"]').exists()).toBe(true);
    expect(w.get('[data-testid="add-service"]').attributes("disabled")).toBeUndefined();
    expect(toastTitles()).toContain("Couldn't add Meilisearch");
  });
});

describe("ServicesView states", () => {
  it("shows the empty state with an Add service button", async () => {
    stubIpc({ services: [svc({ installed_versions: [], selected_version: null })] });
    const w = await mountView();
    const empty = w.get('[data-testid="services-empty"]');
    expect(empty.text()).toContain("No services yet");
    await empty.get("button").trigger("click");
    await flushPromises();
    expect(w.find('[role="dialog"]').exists()).toBe(true);
    expect(calls("addable_service_types")).toHaveLength(1);
  });

  it("opens the Add dialog from a services intent and clears it", async () => {
    stubIpc({});
    servicesIntent.value = "add";
    const w = await mountView();
    expect(w.find('[role="dialog"]').exists()).toBe(true);
    expect(calls("addable_service_types")).toHaveLength(1);
    expect(servicesIntent.value).toBeNull();
  });

  it("shows the load error on a cold failure", async () => {
    stubIpc({ listServices: () => Promise.reject(new Error("daemon down")) });
    const w = await mountView();
    expect(w.text()).toContain("Couldn't load this");
    expect(w.text()).toContain("daemon down");
  });

  it("keeps the list when a forced reload fails over cached data", async () => {
    stubIpc();
    const w = await mountView();
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "list_services" ? Promise.reject(new Error("daemon busy")) : base(cmd, ...rest),
    );
    await useResource("services", listServices, { immediate: false }).refresh({ force: true });
    await flushPromises();
    expect(w.text()).not.toContain("Couldn't load this");
    expect(w.find(`[data-testid="services-list"] [data-key="mysql"]`).exists()).toBe(true);
  });
});

describe("ServicesView drawer layering", () => {
  it("closes only the port modal on Escape over the drawer", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    await select(w, "mysql");
    await button(w, "Edit port").trigger("click");
    await flushPromises();
    escape();
    await flushPromises();
    expect(w.find('input[aria-label="Port"]').exists()).toBe(false);
    expect(w.find('[data-testid="service-inspector"]').exists()).toBe(true);
  });

  it("closes only the logs dialog on Escape over the drawer", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    await select(w, "mysql");
    await inspector(w).get('button[data-action="logs"]').trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="service-log"]').exists()).toBe(true);
    escape();
    await flushPromises();
    expect(w.find('[data-testid="service-log"]').exists()).toBe(false);
    expect(w.find('[data-testid="service-inspector"]').exists()).toBe(true);
  });
});

describe("ServicesView RustFS connect card", () => {
  it("shows the S3 .env block on the instance's port and how to create a bucket", async () => {
    const rustfs = svc({
      service: "rustfs",
      type_id: "rustfs",
      display_name: "RustFS",
      installed_versions: ["1.0.3"],
      selected_version: "1.0.3",
      listen: "127.0.0.1:9002",
      port: 9002,
      supports_databases: false,
    });
    stubIpc({ services: [rustfs] });
    const w = await mountView();

    const card = inspector(w).get('[data-testid="connect-card"]');
    const env = card.get('[data-testid="connect-env"]').text();
    expect(env).toContain("FILESYSTEM_DISK=s3");
    expect(env).toContain("AWS_ACCESS_KEY_ID=rustfsadmin");
    expect(env).toContain("AWS_ENDPOINT=http://127.0.0.1:9002");
    expect(env).toContain("AWS_USE_PATH_STYLE_ENDPOINT=true");
    expect(card.get('[data-testid="rustfs-note"]').text()).toContain("s3 mb s3://your_bucket");
  });
});
