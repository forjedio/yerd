import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import PhpView from "./PhpView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { resetResourceCache, useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { listPhp } from "@/ipc/client";
import type { PhpPoolStatus, PhpUpdate, PhpVersion, StatusReport } from "@/ipc/types";

interface StubOpts {
  installed?: PhpVersion[];
  available?: PhpVersion[];
  legacy?: PhpVersion[];
  extensions?: Record<string, unknown[]>;
  pool?: Record<string, Record<string, string>>;
  settings?: Record<string, string>;
  versionSettings?: Record<string, Record<string, string>>;
  ffiVersions?: PhpVersion[];
  xdebug?: Record<string, unknown>;
  xdebugVersions?: PhpVersion[];
  updates?: PhpUpdate[];
  pools?: Partial<PhpPoolStatus>[];
  jobLog?: string[];
}

/** A default mock: installed 8.4 (default) + legacy 8.1, an available list with
 *  8.5 and legacy 7.4 / 8.0, `ok` for mutations, and a loud reject for anything
 *  unexpected. */
function stubIpc(opts: StubOpts = {}) {
  const installed = opts.installed ?? ["8.1", "8.4"];
  const phpVersions = () => ({
    type: "php_versions",
    installed,
    default: "8.4",
    updates: opts.updates ?? [],
    settings: opts.settings ?? {},
    version_settings: opts.versionSettings ?? {},
    pool: opts.pool ?? {},
    ffi_versions: opts.ffiVersions,
    xdebug: opts.xdebug,
    xdebug_versions: opts.xdebugVersions,
  });
  const pools = (opts.pools ?? []).map((p) => ({
    installed_patch: `${p.version}.3`,
    state: "running",
    pid: 1,
    listen: null,
    rss_bytes: 12 * 1024 * 1024,
    update_available: null,
    ...p,
  }));
  useDaemon().report.value = { php: pools } as unknown as StatusReport;
  let jobPolls = 0;
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_php":
      case "check_php_updates":
        return Promise.resolve(phpVersions());
      case "set_php_settings":
      case "set_php_version_settings":
      case "set_php_pool_settings":
        return Promise.resolve(phpVersions());
      case "list_php_extensions":
        return Promise.resolve({ type: "php_extensions", by_version: opts.extensions ?? {} });
      case "available_php":
        return Promise.resolve({
          type: "available_php",
          available: opts.available ?? ["8.4", "8.5"],
          installed,
          legacy: opts.legacy ?? ["7.4", "8.0"],
        });
      case "install_php_streamed":
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      case "job_status":
        jobPolls += 1;
        return Promise.resolve({
          type: "job_progress",
          state: jobPolls === 1 ? "running" : "succeeded",
          phase: "",
          log: jobPolls === 1 ? (opts.jobLog ?? []) : [],
          next_cursor: jobPolls,
          error: null,
        });
      case "status":
        return Promise.resolve({ type: "status", report: useDaemon().report.value });
      case "restart_php":
      case "set_default_php":
      case "update_php":
      case "uninstall_php":
      case "restart_all_php":
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
    '<div class="menu-item" role="menuitem" :data-disabled="disabled ? \'\' : undefined" @click="$emit(\'select\')"><slot /></div>',
};
const mounted: VueWrapper[] = [];

async function mountView(opts: { realTeleport?: boolean } = {}) {
  const wrapper = mount(PhpView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: !opts.realTeleport,
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

function rowKeys(w: View): string[] {
  return w.findAll('[data-testid="php-list"] [data-key]').map((r) => r.attributes("data-key")!);
}

function row(w: View, key: string) {
  return w.get(`[data-testid="php-list"] [data-key="${key}"]`);
}

async function select(w: View, key: string) {
  await row(w, key).trigger("click");
  await flushPromises();
}

function inspector(w: View) {
  return w.get('[data-testid="php-inspector"]');
}

function panel(w: View, v: string) {
  return w.get(`[data-version-panel="${v}"]`);
}

function visible(el: { element: Element }): boolean {
  return (el.element as HTMLElement).style.display !== "none";
}

function menuItem(w: View, action: string) {
  return w.get(`[data-action="${action}"]`);
}

function called(cmd: string): boolean {
  return invokeMock.mock.calls.some((c) => c[0] === cmd);
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

async function confirmDiscard(w: View) {
  const btn = w
    .get('[role="alertdialog"]')
    .findAll("button")
    .find((b) => b.text().trim() === "Discard");
  await btn!.trigger("click");
  await flushPromises();
}

function escape(): void {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
}

beforeEach(() => {
  invokeMock.mockReset();
  resetResourceCache();
  useDaemon().report.value = null;
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  setWide(true);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  for (const op of [...useOperations().active.value]) useOperations().end(op.id);
});

describe("PhpView list", () => {
  it("lists every version newest first and starts on the default", async () => {
    stubIpc();
    const w = await mountView();
    expect(rowKeys(w)).toEqual(["shared", "8.4", "8.1"]);
    expect(row(w, "8.4").attributes("aria-current")).toBe("true");
    expect(inspector(w).get('[data-testid="version-header"]').text()).toContain("PHP 8.4");
  });

  it("tags the default and legacy versions", async () => {
    stubIpc();
    const w = await mountView();
    expect(row(w, "8.4").find('[data-testid="chip-default"]').exists()).toBe(true);
    expect(row(w, "8.1").find('[data-testid="chip-legacy"]').exists()).toBe(true);
    expect(row(w, "8.4").find('[data-testid="chip-legacy"]').exists()).toBe(false);
  });

  it("shows each version's pool status from the status report", async () => {
    stubIpc({
      installed: ["8.1", "8.3", "8.4"],
      pools: [
        { version: "8.4", state: "running", installed_patch: "8.4.14" },
        { version: "8.3", state: "stopped", installed_patch: "8.3.27" },
        { version: "8.1", state: "failed", installed_patch: "8.1.33" },
      ],
    });
    const w = await mountView();
    const status = (v: string) => row(w, v).get('[data-testid="row-status"]').text();
    expect(status("8.4")).toBe("8.4.14 · running");
    expect(status("8.3")).toBe("8.3.27 · idle");
    expect(status("8.1")).toBe("8.1.33 · failed");
    expect(inspector(w).get('[data-testid="version-status"]').text()).toBe(
      "8.4.14 · FPM pool running · up to date",
    );
  });

  it("marks a configured version customised and passes its pool override to the panel", async () => {
    stubIpc({ pool: { "8.4": { max_children: "32" } } });
    const w = await mountView();
    expect(row(w, "8.4").find('[data-testid="row-customised"]').exists()).toBe(true);
    expect(row(w, "8.1").find('[data-testid="row-customised"]').exists()).toBe(false);
    expect((w.get('input[id="set-8.4-max_children"]').element as HTMLInputElement).value).toBe("32");
    expect((w.get('input[id="set-8.1-max_children"]').element as HTMLInputElement).value).toBe("");
  });

  it("keeps every panel mounted and shows only the selected one", async () => {
    stubIpc();
    const w = await mountView();
    await w.get('input[id="set-8.1-memory_limit"]').setValue("1G");
    expect(visible(panel(w, "8.4"))).toBe(true);
    expect(visible(panel(w, "8.1"))).toBe(false);

    await select(w, "8.1");
    expect(visible(panel(w, "8.1"))).toBe(true);
    expect(visible(panel(w, "8.4"))).toBe(false);
    expect((w.get('input[id="set-8.1-memory_limit"]').element as HTMLInputElement).value).toBe("1G");
    expect(row(w, "8.1").text()).toContain("unsaved changes");
  });

  it("lists an uninstalled version that still has registered extensions", async () => {
    stubIpc({
      installed: ["8.4"],
      extensions: { "8.2": [{ name: "xdebug", path: "/tmp/xdebug.so", zend: true, present: false }] },
    });
    const w = await mountView();
    expect(rowKeys(w)).toContain("8.2");
    expect(w.get('[data-testid="php-list"]').text()).toContain("Not installed");
    expect(row(w, "8.2").text()).toContain("not installed · 1 extension");
  });

  it("has no version rows when nothing is installed or registered and selects Shared defaults", async () => {
    stubIpc({ installed: [] });
    const w = await mountView();
    expect(rowKeys(w)).toEqual(["shared"]);
    expect(visible(w.get('[data-testid="shared-defaults"]'))).toBe(true);
  });
});

describe("PhpView updates", () => {
  const one: PhpUpdate[] = [{ version: "8.4", installed: "8.4.14", latest: "8.4.15" }];

  it("shows the update chip, the banner and updates the one version", async () => {
    stubIpc({ updates: one });
    const w = await mountView();
    expect(row(w, "8.4").get('[data-testid="chip-update"]').text()).toBe("↑ 8.4.15");
    const banner = w.get('[data-testid="update-banner"]');
    expect(banner.text()).toContain("1 update available");
    expect(banner.text()).toContain("PHP 8.4.14 → 8.4.15");
    await banner.get('[data-testid="banner-update"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("update_php", { version: "8.4" });
    expect(toastTitles()).toContain("Updated PHP 8.4");
  });

  it("updates all from the banner when several are pending", async () => {
    stubIpc({
      updates: [...one, { version: "8.1", installed: "8.1.1", latest: "8.1.2" }],
    });
    const w = await mountView();
    const banner = w.get('[data-testid="update-banner"]');
    expect(banner.text()).toContain("2 updates available");
    expect(banner.get('[data-testid="banner-update"]').text()).toBe("Update all");
    await banner.get('[data-testid="banner-update"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("update_php", { version: null });
  });

  it("has no banner without updates", async () => {
    stubIpc();
    const w = await mountView();
    expect(w.find('[data-testid="update-banner"]').exists()).toBe(false);
  });

  it("applies the Xdebug state from the update check without a second list call", async () => {
    stubIpc({ installed: ["8.4"], xdebugVersions: ["8.4"] });
    let finishCheck: (r: unknown) => void = () => {};
    const w = await mountView();
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "check_php_updates"
        ? new Promise((resolve) => {
            finishCheck = resolve;
          })
        : base(cmd, ...rest),
    );
    const listCalls = () => invokeMock.mock.calls.filter(([c]) => c === "list_php").length;
    const before = listCalls();
    expect(row(w, "8.4").text()).not.toContain("Xdebug on");

    const check = button(w, "Check for updates");
    await check.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("check_php_updates", undefined);
    expect(check.attributes("disabled")).toBeDefined();

    finishCheck({
      type: "php_versions",
      installed: ["8.4"],
      default: "8.4",
      updates: [],
      settings: {},
      xdebug: { "8.4": { enabled: true, modes: ["debug"], start: "always" } },
      xdebug_versions: ["8.4"],
    });
    await flushPromises();

    expect(listCalls()).toBe(before);
    expect(check.attributes("disabled")).toBeUndefined();
    expect(row(w, "8.4").text()).toContain("Xdebug on");
    expect(w.get('button[aria-label="Xdebug for PHP 8.4"]').attributes("aria-checked")).toBe("true");
    expect(toastTitles()).toContain("Update check complete");
  });
});

describe("PhpView version actions", () => {
  it("restarts a running pool and disables Restart for an idle one", async () => {
    stubIpc({
      pools: [
        { version: "8.4", state: "running" },
        { version: "8.1", state: "stopped" },
      ],
    });
    const w = await mountView();
    await button(w, "Restart pool").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("restart_php", { version: "8.4" });

    await select(w, "8.1");
    expect(button(w, "Restart pool").attributes("disabled")).toBeDefined();
  });

  it("makes a version the default, but not the current default or a legacy one", async () => {
    stubIpc({ installed: ["8.1", "8.3", "8.4"] });
    const w = await mountView();
    expect(menuItem(w, "make-default").attributes("data-disabled")).toBeDefined();
    await select(w, "8.1");
    expect(menuItem(w, "make-default").attributes("data-disabled")).toBeDefined();

    await select(w, "8.3");
    expect(menuItem(w, "make-default").attributes("data-disabled")).toBeUndefined();
    await menuItem(w, "make-default").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_default_php", { version: "8.3" });
  });

  it("offers Update only with an update available", async () => {
    stubIpc({ updates: [{ version: "8.4", installed: "8.4.14", latest: "8.4.15" }] });
    const w = await mountView();
    expect(menuItem(w, "update").text()).toContain("Update to 8.4.15");
    await select(w, "8.1");
    expect(w.find('[data-action="update"]').exists()).toBe(false);
  });

  it("does nothing when the uninstall confirm is cancelled", async () => {
    stubIpc();
    const w = await mountView();
    await menuItem(w, "uninstall").trigger("click");
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Uninstall PHP 8.4?");
    await button(w, "Cancel").trigger("click");
    await flushPromises();
    expect(called("uninstall_php")).toBe(false);
  });

  it("reloads the list after a failed uninstall and keeps the error toast", async () => {
    stubIpc();
    const w = await mountView();
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "uninstall_php"
        ? Promise.reject(new Error("pool for PHP 8.1 is still in use"))
        : base(cmd, ...rest),
    );
    const listCalls = () => invokeMock.mock.calls.filter(([c]) => c === "list_php").length;
    await select(w, "8.1");
    const before = listCalls();

    await menuItem(w, "uninstall").trigger("click");
    await flushPromises();
    await button(w, "Uninstall").trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("uninstall_php", { version: "8.1" });
    expect(listCalls()).toBeGreaterThan(before);
    expect(toastTitles()).toContain("Couldn't uninstall PHP 8.1");
  });
});

describe("PhpView busy and uninstall bookkeeping", () => {
  it("runs one update at a time and keeps it disabled while another action finishes", async () => {
    stubIpc({ updates: [{ version: "8.4", installed: "8.4.14", latest: "8.4.15" }], pools: [{ version: "8.4" }] });
    const w = await mountView();
    let finishUpdate: () => void = () => {};
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "update_php"
        ? new Promise((resolve) => {
            finishUpdate = () => resolve({ type: "ok" });
          })
        : base(cmd, ...rest),
    );
    const updates = () => invokeMock.mock.calls.filter(([c]) => c === "update_php").length;

    await w.get('[data-testid="banner-update"]').trigger("click");
    await flushPromises();
    expect(menuItem(w, "update").attributes("data-disabled")).toBeDefined();

    await button(w, "Restart pool").trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="banner-update"]').attributes("disabled")).toBeDefined();
    expect(menuItem(w, "update").attributes("data-disabled")).toBeDefined();

    await menuItem(w, "update").trigger("click");
    await w.get('[data-testid="banner-update"]').trigger("click");
    await flushPromises();
    expect(updates()).toBe(1);

    finishUpdate();
    await flushPromises();
    expect(toastTitles().filter((t) => t === "Updated PHP 8.4")).toHaveLength(1);
  });

  it("forgets a dirty version once it is uninstalled", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    await select(w, "8.1");
    await w.get('input[id="set-8.1-memory_limit"]').setValue("1G");
    stubIpc({ installed: ["8.4"] });

    await menuItem(w, "uninstall").trigger("click");
    await flushPromises();
    await button(w, "Uninstall").trigger("click");
    await flushPromises();
    expect(rowKeys(w)).not.toContain("8.1");

    await w.get('button[aria-label="Close PHP details"]').trigger("click");
    await flushPromises();
    expect(w.find('[role="alertdialog"]').exists()).toBe(false);
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);
  });
});

describe("PhpView install", () => {
  async function openInstall(w: View) {
    await w.get('[data-testid="install-php"]').trigger("click");
    await flushPromises();
  }

  it("installs from the dialog and shows the parsed download progress", async () => {
    stubIpc({ jobLog: ["Found PHP 8.5.1 — downloading…", "Downloading CLI: 18.0 MB / 42.0 MB"] });
    const w = await mountView();
    await openInstall(w);
    await w.get('[data-testid="install-submit"]').trigger("click");
    await flushPromises();

    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ version: "8.5", confirmLegacy: false });
    expect(w.get('[data-testid="install-line"]').text()).toBe("Downloading CLI — 18.0 of 42.0 MB");
    expect(w.get('[data-testid="install-progress"]').text()).toContain("42%");
    expect(w.get('[data-testid="install-php"]').attributes("disabled")).toBeDefined();

    await button(w, "Close").trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="install-progress"]').exists()).toBe(false);

    await vi.waitFor(() => expect(toastTitles()).toContain("Installed PHP 8.5"), { timeout: 2000 });
    expect(w.get('[data-testid="install-php"]').attributes("disabled")).toBeUndefined();
  });

  it("shows a non-download line as text over an indeterminate bar", async () => {
    stubIpc({ jobLog: ["Resolving PHP 8.5…"] });
    const w = await mountView();
    await openInstall(w);
    await w.get('[data-testid="install-submit"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="install-line"]').text()).toBe("Resolving PHP 8.5…");
    expect(w.find('[data-testid="install-bar-indeterminate"]').exists()).toBe(true);
    await vi.waitFor(() => expect(toastTitles()).toContain("Installed PHP 8.5"), { timeout: 2000 });
  });

  it("passes the legacy acknowledgement through", async () => {
    stubIpc({ available: ["8.4"] });
    const w = await mountView();
    await openInstall(w);
    await w.get('[data-testid="legacy-ack"]').setValue(true);
    await w.get('[data-testid="install-submit"]').trigger("click");
    await flushPromises();
    const streamed = invokeMock.mock.calls.find((c) => c[0] === "install_php_streamed");
    expect(streamed?.[1]).toMatchObject({ version: "8.0", confirmLegacy: true });
    await vi.waitFor(() => expect(toastTitles()).toContain("Installed PHP 8.0"), { timeout: 2000 });
  });
});

describe("PhpView layouts", () => {
  it("shows the inspector beside the list on a wide window", async () => {
    stubIpc();
    const w = await mountView();
    expect(w.get('[data-testid="php-list"]').attributes("data-layout")).toBe("list");
    expect(inspector(w).attributes("role")).toBeUndefined();
  });

  it("opens a version or Shared defaults in the drawer from the compact table", async () => {
    setWide(null);
    stubIpc({
      installed: ["8.1", "8.3", "8.4"],
      xdebug: { "8.4": { enabled: true, modes: ["debug"], start: "always" } },
      xdebugVersions: ["8.3", "8.4"],
    });
    const w = await mountView();
    expect(w.get('[data-testid="php-list"]').attributes("data-layout")).toBe("table");
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);
    const xdebug = (v: string) => row(w, v).get('[data-testid="cell-xdebug"]').text();
    expect(xdebug("8.4")).toBe("Xdebug on");
    expect(xdebug("8.3")).toBe("off");
    expect(xdebug("8.1")).toBe("unavailable");

    await select(w, "8.3");
    expect(inspector(w).attributes("role")).toBe("dialog");
    expect(inspector(w).get('[data-testid="version-header"]').text()).toContain("PHP 8.3");
    expect(w.get('button[aria-label="Xdebug for PHP 8.4"]').attributes("aria-checked")).toBe("true");

    await w.get('button[aria-label="Close PHP details"]').trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);

    await select(w, "shared");
    expect(visible(inspector(w).get('[data-testid="shared-defaults"]'))).toBe(true);
  });

  it("asks before a dirty drawer closes and names every unsaved item", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    await select(w, "8.1");
    await w.get('input[id="set-8.1-memory_limit"]').setValue("1G");
    await select(w, "8.4");
    await w.get('input[id="set-8.4-memory_limit"]').setValue("2G");

    escape();
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Discard unsaved changes?");
    expect(dialog.text()).toContain("Your edits to PHP 8.4 and PHP 8.1 haven't been saved.");

    await button(w, "Cancel").trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(true);
    expect((w.get('input[id="set-8.4-memory_limit"]').element as HTMLInputElement).value).toBe("2G");

    await w.get('button[aria-label="Close PHP details"]').trigger("click");
    await flushPromises();
    await confirmDiscard(w);
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);
  });

  it("closes a clean drawer at once", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    await select(w, "8.4");
    escape();
    await flushPromises();
    expect(w.find('[role="alertdialog"]').exists()).toBe(false);
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);
  });

  it("leaves Escape to an open extension menu in the drawer", async () => {
    setWide(null);
    stubIpc({
      extensions: { "8.4": [{ name: "scrypt", path: "/x/scrypt.so", zend: false, present: true }] },
    });
    const w = await mountView();
    await select(w, "8.4");
    const extMenu = panel(w, "8.4")
      .findAllComponents({ name: "MenuStub" })
      .find((m: VueWrapper) => m.text().includes("Add ini directive"))!;
    extMenu.vm.$emit("update:open", true);
    await flushPromises();
    escape();
    await flushPromises();
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(true);
  });

  it("opens the drawer with the edit intact when a dirty wide layout turns compact", async () => {
    stubIpc();
    const w = await mountView({ realTeleport: true });
    await w.get('input[id="set-8.4-memory_limit"]').setValue("1G");
    const field = w.get('input[id="set-8.4-memory_limit"]').element;
    mediaListener?.({ matches: false });
    await flushPromises();
    const drawer = document.body.querySelector('[data-testid="php-inspector"]');
    expect(drawer?.getAttribute("role")).toBe("dialog");
    const after = document.body.querySelector<HTMLInputElement>('input[id="set-8.4-memory_limit"]');
    expect(after).toBe(field);
    expect(after?.value).toBe("1G");
  });

  it("keeps the list and staged edits when a forced reload fails over cached data", async () => {
    stubIpc();
    const w = await mountView();
    await w.get('input[id="set-8.4-memory_limit"]').setValue("1G");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "list_php" ? Promise.reject(new Error("daemon busy")) : base(cmd, ...rest),
    );
    await useResource("php", listPhp, { immediate: false }).refresh({ force: true });
    await flushPromises();
    expect(w.text()).not.toContain("Couldn't load this");
    expect((w.get('input[id="set-8.4-memory_limit"]').element as HTMLInputElement).value).toBe("1G");
  });

  it("keeps a save that lands after the drawer was discarded", async () => {
    setWide(null);
    stubIpc();
    const w = await mountView();
    let finish: (r: unknown) => void = () => {};
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "set_php_version_settings"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : base(cmd, ...rest),
    );
    await select(w, "8.4");
    await w.get('input[id="set-8.4-memory_limit"]').setValue("1G");
    await w.get('[data-version-panel="8.4"] [data-testid="version-save"]').trigger("click");
    await flushPromises();
    await w.get('button[aria-label="Close PHP details"]').trigger("click");
    await flushPromises();
    await confirmDiscard(w);
    expect(w.find('[data-testid="php-inspector"]').exists()).toBe(false);

    const saved = {
      type: "php_versions",
      installed: ["8.1", "8.4"],
      default: "8.4",
      settings: {},
      version_settings: { "8.4": { memory_limit: "1G" } },
    };
    invokeMock.mockImplementation((cmd: string, ...rest: unknown[]) =>
      cmd === "list_php" ? Promise.resolve(saved) : base(cmd, ...rest),
    );
    finish(saved);
    await flushPromises();
    expect(useResource("php", listPhp, { immediate: false }).data.value).toMatchObject({
      version_settings: { "8.4": { memory_limit: "1G" } },
    });

    await select(w, "8.4");
    expect((w.get('input[id="set-8.4-memory_limit"]').element as HTMLInputElement).value).toBe("1G");
  });
});
