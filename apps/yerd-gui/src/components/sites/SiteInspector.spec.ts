import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { computed, nextTick, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openInBrowser = vi.fn();
const openPath = vi.fn();
const openInTerminal = vi.fn();
const openInIde = vi.fn();
const openInSystemDefault = vi.fn();
const getInstalledIdes = vi.fn();
const getPreferredIde = vi.fn();
const getSiteIdeOverrides = vi.fn();
const setSiteIdeOverride = vi.fn();
const pickDirectory = vi.fn();
const showDumpsWindow = vi.fn();
const mintWordPressLoginToken = vi.fn();
const wordpressAdminUsers = vi.fn();
const addDomain = vi.fn();
const removeDomain = vi.fn();
const setPrimaryDomain = vi.fn();
const resetDomains = vi.fn();
const listRoutes = vi.fn();
const addRouteRule = vi.fn();
const removeRouteRule = vi.fn();
const listResponseHeaders = vi.fn();
const setResponseHeader = vi.fn();
const removeResponseHeader = vi.fn();
const clipboardWriteText = vi.fn();
vi.mock("@/ipc/client", () => ({
  openInBrowser: (...args: unknown[]) => openInBrowser(...args),
  openPath: (...args: unknown[]) => openPath(...args),
  openInTerminal: (...args: unknown[]) => openInTerminal(...args),
  openInIde: (...args: unknown[]) => openInIde(...args),
  openInSystemDefault: (...args: unknown[]) => openInSystemDefault(...args),
  getInstalledIdes: (...args: unknown[]) => getInstalledIdes(...args),
  getPreferredIde: (...args: unknown[]) => getPreferredIde(...args),
  getSiteIdeOverrides: (...args: unknown[]) => getSiteIdeOverrides(...args),
  setSiteIdeOverride: (...args: unknown[]) => setSiteIdeOverride(...args),
  pickDirectory: (...args: unknown[]) => pickDirectory(...args),
  showDumpsWindow: (...args: unknown[]) => showDumpsWindow(...args),
  mintWordPressLoginToken: (...args: unknown[]) => mintWordPressLoginToken(...args),
  wordpressAdminUsers: (...args: unknown[]) => wordpressAdminUsers(...args),
  addDomain: (...args: unknown[]) => addDomain(...args),
  removeDomain: (...args: unknown[]) => removeDomain(...args),
  setPrimaryDomain: (...args: unknown[]) => setPrimaryDomain(...args),
  resetDomains: (...args: unknown[]) => resetDomains(...args),
  listRoutes: (...args: unknown[]) => listRoutes(...args),
  addRouteRule: (...args: unknown[]) => addRouteRule(...args),
  removeRouteRule: (...args: unknown[]) => removeRouteRule(...args),
  listResponseHeaders: (...args: unknown[]) => listResponseHeaders(...args),
  setResponseHeader: (...args: unknown[]) => setResponseHeader(...args),
  removeResponseHeader: (...args: unknown[]) => removeResponseHeader(...args),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

vi.mock("vue-router", () => ({
  useRouter: () => ({ push: vi.fn() }),
}));

const hostPlatform = ref("linux");
vi.mock("@/composables/usePlatform", () => ({
  loadPlatform: () => Promise.resolve(),
  usePlatform: () => ({
    platform: hostPlatform,
    isMac: computed(() => hostPlatform.value === "macos"),
    isLinux: computed(() => hostPlatform.value === "linux"),
    supportsPathInstall: computed(
      () => hostPlatform.value === "macos" || hostPlatform.value === "linux",
    ),
    home: ref("/Users/a"),
  }),
}));

import { resetIdes } from "@/composables/useIdes";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";
import SiteInspector from "./SiteInspector.vue";

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "blog",
    document_root: "/srv/blog",
    php: "8.3",
    secure: true,
    kind: "linked",
    is_laravel: true,
    framework: "laravel",
    uses_front_controller: true,
    ...overrides,
  };
}

function wpSite(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return site({
    is_laravel: false,
    framework: "wordpress",
    is_wordpress: true,
    uses_front_controller: undefined,
    wp_auto_login: false,
    ...overrides,
  });
}

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ca: { path: "", fingerprint: "", trusted_system: true },
    ...overrides,
  } as StatusReport;
}

const GROUPS: GroupsState = { order: ["Client work", "Personal"], members: {} };

const Pass = { template: "<div><slot /></div>" };
const stubs = {
  teleport: true,
  SiteNodePanel: {
    emits: ["changed"],
    template: '<button data-testid="node-stub" @click="$emit(\'changed\')">node</button>',
  },
  DropdownMenu: {
    props: ["open"],
    emits: ["update:open"],
    template: '<div data-testid="dropdown"><slot /></div>',
  },
  DropdownMenuTrigger: Pass,
  DropdownMenuContent: Pass,
  DropdownMenuSub: Pass,
  DropdownMenuSubTrigger: Pass,
  DropdownMenuSubContent: Pass,
  DropdownMenuItem: {
    props: ["disabled"],
    emits: ["select"],
    template: '<div class="menu-item" role="menuitem" @click="$emit(\'select\')"><slot /></div>',
  },
  DropdownMenuSeparator: { template: "<hr />" },
  PopoverRoot: {
    props: ["open"],
    emits: ["update:open"],
    template: '<div data-testid="popover-root"><slot /></div>',
  },
  PopoverAnchor: Pass,
  PopoverPortal: Pass,
  PopoverContent: Pass,
};

const mounted: VueWrapper[] = [];

function mountInspector(
  s: SiteEntry | null = site(),
  extraProps: Record<string, unknown> = {},
  options: { attachTo?: Element } = {},
) {
  const wrapper = mount(SiteInspector, {
    props: {
      site: s,
      mode: "drawer",
      open: true,
      report: report(),
      tld: "test",
      home: "/Users/a",
      phpVersions: ["8.3", "8.4"],
      defaultPhp: "8.4",
      groups: GROUPS,
      currentGroup: "",
      parentDir: null,
      ...extraProps,
    },
    global: { stubs },
    ...options,
  });
  mounted.push(wrapper);
  return wrapper;
}

type Inspector = ReturnType<typeof mountInspector>;

function button(w: Inspector, text: string) {
  const b = w.findAll("button").find((x) => x.text() === text);
  if (!b) throw new Error(`no "${text}" button`);
  return b;
}

function menuItems(w: Inspector): string[] {
  return w.findAll(".menu-item").map((i) => i.text());
}

async function pick(w: Inspector, text: string) {
  const item = w.findAll(".menu-item").find((i) => i.text().includes(text));
  if (!item) throw new Error(`no "${text}" menu item`);
  await item.trigger("click");
  await nextTick();
  await flushPromises();
}

beforeEach(() => {
  vi.clearAllMocks();
  getInstalledIdes.mockResolvedValue([]);
  getPreferredIde.mockResolvedValue(null);
  getSiteIdeOverrides.mockResolvedValue({});
  setSiteIdeOverride.mockResolvedValue(undefined);
  wordpressAdminUsers.mockResolvedValue([]);
  addDomain.mockResolvedValue(undefined);
  listRoutes.mockResolvedValue([]);
  removeRouteRule.mockResolvedValue(undefined);
  listResponseHeaders.mockResolvedValue([]);
  removeResponseHeader.mockResolvedValue(undefined);
  clipboardWriteText.mockResolvedValue(undefined);
  hostPlatform.value = "linux";
  resetIdes();
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
});

afterEach(() => {
  mounted.splice(0).forEach((w) => w.unmount());
  document.body.innerHTML = "";
});

describe("SiteInspector header and actions", () => {
  it("renders the site's host and its served path relative to home", () => {
    const w = mountInspector(
      site({ document_root: "/Users/a/Code/blog", web_subpath: "public" }),
    );
    expect(w.get("h2").text()).toBe("blog.test");
    expect(w.get('[data-testid="inspector-path"]').text()).toBe("~/Code/blog/public");
  });

  it("opens the site, a terminal and the folder", async () => {
    hostPlatform.value = "macos";
    const w = mountInspector();
    await button(w, "Open site").trigger("click");
    expect(openInBrowser).toHaveBeenCalledWith("https://blog.test");
    await button(w, "Terminal").trigger("click");
    expect(openInTerminal).toHaveBeenCalledWith("/srv/blog");
    await button(w, "Finder").trigger("click");
    expect(openPath).toHaveBeenCalledWith("/srv/blog");
  });

  it("calls the folder button Files off macOS", () => {
    const w = mountInspector();
    expect(button(w, "Files").exists()).toBe(true);
  });

  it("reflects and toggles the favourite star", async () => {
    const off = mountInspector();
    const star = off.get('[aria-label="Star blog"]');
    expect(star.attributes("aria-pressed")).toBe("false");
    await star.trigger("click");
    expect(off.emitted("toggleFavourite")).toEqual([[site()]]);

    const on = mountInspector(site({ favourite: true }));
    expect(on.get('[aria-label="Unstar blog"]').attributes("aria-pressed")).toBe("true");
  });

  it("copies the URL and highlights a non-default port", async () => {
    const w = mountInspector(site(), {
      report: report({ https: { requested: 443, bound: 8443, fell_back: true } }),
    });
    const row = w.get('[data-testid="inspector-url"]');
    expect(row.text()).toContain("https://blog.test:8443");
    expect(row.get(".text-warning").text()).toBe(":8443");
    await row.get('[aria-label="Copy URL"]').trigger("click");
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenCalledWith("https://blog.test:8443");
    expect(toastSuccess).toHaveBeenCalledWith("Copied URL", undefined);
  });

  it.each([
    [site(), report(), "Serving"],
    [site({ apex_shadowed_by: "other" }), report(), "Served by other"],
    [site(), report({ web_unbound: { http: 8080, https: 8443 } }), "Not serving"],
  ])("shows the serving pill (%#)", (s, r, label) => {
    const w = mountInspector(s, { report: r });
    expect(w.get('[data-testid="serving-pill"]').text()).toBe(label);
  });

  it("shares from the menu", async () => {
    const w = mountInspector();
    await pick(w, "Share publicly");
    expect(w.emitted("share")).toEqual([[site()]]);
  });
});

describe("SiteInspector environment", () => {
  it("emits a PHP change from the segmented control", async () => {
    const w = mountInspector();
    const radio = w
      .get('[aria-label="Site PHP version"]')
      .findAll('[role="radio"]')
      .find((r) => r.text() === "8.4");
    await radio?.trigger("click");
    expect(w.emitted("changePhp")).toEqual([[site(), "8.4"]]);
    expect(w.text()).toContain("Global default is 8.4");
  });

  it("toggles HTTPS and words the certificate by trust", async () => {
    const w = mountInspector();
    await w.get('[aria-label="HTTPS"]').trigger("click");
    expect(w.emitted("toggleSecure")).toEqual([[site()]]);
    const subtitle = (s: SiteEntry, trusted: boolean | null) =>
      mountInspector(s, {
        report: report({ ca: { path: "", fingerprint: "", trusted_system: trusted } }),
      })
        .get('[data-testid="https-subtitle"]')
        .text();
    expect(subtitle(site(), true)).toBe("Certificate issued by Yerd local CA · trusted");
    expect(subtitle(site(), false)).toBe("Certificate issued by Yerd local CA · not trusted");
    expect(subtitle(site(), null)).toBe("Certificate issued by Yerd local CA · trust unknown");
    expect(subtitle(site({ secure: false }), true)).toBe("Served over plain HTTP");
  });

  it("describes the framework and the source", () => {
    const w = mountInspector(site({ kind: "parked", web_subpath: "public", document_root: "/Users/a/Herd/blog" }), {
      parentDir: "/Users/a/Herd",
    });
    expect(w.get('[data-testid="framework-label"]').text()).toBe("Laravel");
    expect(w.text()).toContain("Detected · front controller at /public");
    expect(w.get('[data-testid="source-chip"]').text()).toBe("Parked");
    expect(w.text()).toContain("Parked in ~/Herd, served automatically");
  });

  it("forwards a Node change", async () => {
    const w = mountInspector();
    await w.get('[data-testid="node-stub"]').trigger("click");
    expect(w.emitted("nodeChanged")).toHaveLength(1);
  });
});

describe("SiteInspector domains", () => {
  it("lists the domains with their tags and the shadowed note", () => {
    const w = mountInspector(
      site({ domains: ["blog.test", "*.blog.test"], apex_shadowed_by: "other" }),
    );
    const card = w.get('[data-testid="domains-card"]');
    expect(card.text()).toContain("blog.test");
    expect(card.text()).toContain("primary");
    expect(card.text()).toContain("wildcard");
    expect(card.get('[data-testid="domains-shadowed"]').text()).toContain("served by “other”");
  });

  it("manages domains in the shared panel", async () => {
    const w = mountInspector(site({ domains: ["blog.test", "api.blog.test"] }));
    await button(w, "Add domain").trigger("click");
    await nextTick();
    expect(w.text()).toContain("Domains · blog.test");
    await w.get("#add-domain").setValue("shop.blog.test");
    const add = w.findAll("button").find((b) => b.text() === "Add");
    await add?.trigger("click");
    await flushPromises();
    expect(addDomain).toHaveBeenCalledWith("blog", "shop.blog.test");
    expect(w.emitted("domainsChanged")).toHaveLength(1);
  });
});

describe("SiteInspector menu", () => {
  it("offers web root and Dumps for a non-WordPress linked site", () => {
    const w = mountInspector();
    const items = menuItems(w);
    expect(items.some((t) => t.includes("Change web root"))).toBe(true);
    expect(items.some((t) => t.includes("Dumps viewer"))).toBe(true);
    expect(items.some((t) => t.includes("WP Admin"))).toBe(false);
    expect(items.some((t) => t.includes("Remove site"))).toBe(true);
    expect(items.some((t) => t.includes("Unpark"))).toBe(false);
  });

  it("offers WP Admin and no web root for WordPress", () => {
    const items = menuItems(mountInspector(wpSite()));
    expect(items.some((t) => t.includes("WP Admin"))).toBe(true);
    expect(items.some((t) => t.includes("Dumps viewer"))).toBe(false);
    expect(items.some((t) => t.includes("Change web root"))).toBe(false);
  });

  it("offers Unpark of the parent folder for a parked site", async () => {
    const w = mountInspector(site({ kind: "parked" }), { parentDir: "/Users/a/Herd" });
    expect(menuItems(w)).toContain("Unpark ~/Herd…");
    expect(menuItems(w).some((t) => t.includes("Remove site"))).toBe(false);
    await pick(w, "Unpark ~/Herd");
    expect(w.emitted("unpark")).toEqual([["/Users/a/Herd"]]);
  });

  it("removes a linked site from the menu", async () => {
    const w = mountInspector();
    await pick(w, "Remove site");
    expect(w.emitted("unlink")).toEqual([[site()]]);
  });

  it("opens the Dumps viewer", async () => {
    const w = mountInspector();
    await pick(w, "Dumps viewer");
    expect(showDumpsWindow).toHaveBeenCalledOnce();
  });

  it("copies the URL from the menu", async () => {
    const w = mountInspector();
    await pick(w, "Copy URL");
    expect(clipboardWriteText).toHaveBeenCalledWith("https://blog.test");
  });

  it("opens the plain WP Admin link when auto-login is off", async () => {
    const w = mountInspector(wpSite({ wp_auto_login: false }));
    await pick(w, "WP Admin");
    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("https://blog.test/wp-admin/");
  });

  it("mints a pre-authenticated WP Admin link when auto-login is on", async () => {
    mintWordPressLoginToken.mockResolvedValue("sekrit-token");
    const w = mountInspector(wpSite({ wp_auto_login: true }));
    await pick(w, "WP Admin");
    expect(openInBrowser).toHaveBeenCalledWith(
      "https://blog.test/wp-admin/?yerd_login_token=sekrit-token",
    );
  });

  it("checks the current group and moves the site", async () => {
    const w = mountInspector(site(), { currentGroup: "Personal" });
    const personal = w.get('[data-group="Personal"]');
    expect(personal.find("svg").classes()).not.toContain("opacity-0");
    expect(w.get('[data-group="Client work"]').find("svg").classes()).toContain("opacity-0");
    await w.get('[data-group="Client work"]').trigger("click");
    expect(w.emitted("changeGroup")).toEqual([[site(), "Client work"]]);
    await w.get('[data-group=""]').trigger("click");
    expect(w.emitted("changeGroup")?.[1]).toEqual([site(), ""]);
  });

  it("validates a new group and creates and moves", async () => {
    const w = mountInspector();
    await pick(w, "New group");
    const popover = w.get('[data-testid="new-group-popover"]');
    const submit = popover.findAll("button").find((b) => b.text() === "Create & move");
    const input = popover.get('[aria-label="New group name"]');
    for (const bad of ["", "  ", "unallocated", "client WORK"]) {
      await input.setValue(bad);
      expect(submit?.attributes("disabled")).toBeDefined();
    }
    await input.setValue("Side projects");
    expect(submit?.attributes("disabled")).toBeUndefined();
    await popover.trigger("submit");
    expect(w.emitted("createGroupAndMove")).toEqual([[site(), "Side projects"]]);
  });

  it("lists editors, checks the current one and stores a pick", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "vscode", label: "VS Code" },
      { id: "zed", label: "Zed" },
    ]);
    getSiteIdeOverrides.mockResolvedValue({ blog: "zed" });
    const w = mountInspector();
    await flushPromises();
    const ide = (id: string) => w.get(`[data-ide="${id}"]`);
    expect(ide("default").text()).toBe("Use default (VS Code)");
    expect(ide("zed").find("svg").classes()).not.toContain("opacity-0");
    await ide("vscode").trigger("click");
    await flushPromises();
    expect(setSiteIdeOverride).toHaveBeenCalledWith("blog", "vscode");
    await button(w, "Editor").trigger("click");
    expect(openInIde).toHaveBeenCalledWith("blog", "vscode");
  });

  it("hides the Editor button and submenu on a platform with no launcher", async () => {
    hostPlatform.value = "windows";
    const w = mountInspector();
    await flushPromises();
    expect(w.findAll("button").some((b) => b.text() === "Editor")).toBe(false);
    expect(w.find("[data-ide]").exists()).toBe(false);
  });
});

describe("SiteInspector web root dialog", () => {
  it("converts a picked directory to a relative web root and saves it", async () => {
    pickDirectory.mockResolvedValue("/srv/blog/public");
    const w = mountInspector();
    await pick(w, "Change web root");
    await w.get('[aria-label="Choose web root directory"]').trigger("click");
    await flushPromises();
    expect(pickDirectory).toHaveBeenCalledWith("/srv/blog");
    expect((w.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe(
      "public",
    );
    await button(w, "Save").trigger("click");
    expect(w.emitted("changeWebRoot")).toEqual([[site(), "public"]]);
  });

  it("rejects a picked directory outside the site folder", async () => {
    pickDirectory.mockResolvedValue("/srv/other");
    const w = mountInspector();
    await pick(w, "Change web root");
    await w.get('[aria-label="Choose web root directory"]').trigger("click");
    await flushPromises();
    expect((w.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe("");
    expect(toastError).toHaveBeenCalledWith(
      "Invalid web root",
      "Choose a directory inside the site folder.",
    );
  });

  it("copies the current web root and keeps Save off while unchanged", async () => {
    const w = mountInspector(site({ web_subpath: "web" }));
    await pick(w, "Change web root");
    expect(button(w, "Save").attributes("disabled")).toBeDefined();
    await w.get('[aria-label="Copy web root"]').trigger("click");
    expect(clipboardWriteText).toHaveBeenCalledWith("web");
  });

  it("drops an unsaved edit when reopened", async () => {
    const w = mountInspector();
    await pick(w, "Change web root");
    await w.get('[aria-label="Site web root"]').setValue("public");
    await button(w, "Cancel").trigger("click");
    await pick(w, "Change web root");
    expect((w.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe("");
  });
});

describe("SiteInspector routing and headers", () => {
  it("toggles the front controller and manages routing rules", async () => {
    listRoutes.mockResolvedValue([{ site: "blog", prefix: "/api", target: "api/index.php" }]);
    const w = mountInspector();
    await pick(w, "Routing rules");
    expect(w.text()).toContain("Routing rules · blog.test");
    await w.get('[aria-label="Route through front controller"]').trigger("click");
    expect(w.emitted("toggleFrontController")).toEqual([[site(), false]]);
    expect(w.text()).toContain("api/index.php");
    await w.get('[aria-label="Remove route /api"]').trigger("click");
    await flushPromises();
    expect(removeRouteRule).toHaveBeenCalledWith("blog", "/api");
  });

  it("hides the front-controller switch for WordPress", async () => {
    const w = mountInspector(wpSite({ uses_front_controller: true }));
    await pick(w, "Routing rules");
    expect(w.find('[aria-label="Route through front controller"]').exists()).toBe(false);
  });

  it("manages response headers", async () => {
    listResponseHeaders.mockResolvedValue([
      { site: "blog", name: "Access-Control-Allow-Origin", value: "https://app.test" },
    ]);
    const w = mountInspector();
    await pick(w, "Response headers");
    expect(w.text()).toContain("Access-Control-Allow-Origin:");
    await w.get('[aria-label="Remove header Access-Control-Allow-Origin"]').trigger("click");
    await flushPromises();
    expect(removeResponseHeader).toHaveBeenCalledWith("blog", "Access-Control-Allow-Origin");
  });
});

describe("SiteInspector footer", () => {
  it.each([
    [site({ kind: "parked" }), "/Users/a/Herd", "Unpark site…", "unpark"],
    [site(), null, "Remove site…", "unlink"],
  ] as const)("offers %#: %s", async (s, parentDir, label, event) => {
    const w = mountInspector(s, { parentDir });
    const action = w.get('[data-testid="inspector-footer-action"]');
    expect(action.text()).toBe(label);
    await action.trigger("click");
    expect(w.emitted(event)).toHaveLength(1);
  });

  it("offers nothing for a parked site outside any parked root", () => {
    const w = mountInspector(site({ kind: "parked" }), { parentDir: null });
    expect(w.find('[data-testid="inspector-footer-action"]').exists()).toBe(false);
  });
});

describe("SiteInspector drawer", () => {
  it("closes on Escape, the backdrop and its X", async () => {
    const w = mountInspector();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await w.get(".site-inspector-backdrop").trigger("click");
    await w.get('[aria-label="Close site details"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(3);
  });

  it("leaves Escape to a confirm dialog layered above it", () => {
    const w = mountInspector();
    const above = document.createElement("div");
    above.setAttribute("role", "alertdialog");
    document.body.appendChild(above);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.emitted("close")).toBeUndefined();
    above.remove();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.emitted("close")).toHaveLength(1);
  });

  it.each(["Routing rules", "Change web root"])(
    "closes only the %s dialog on Escape",
    async (entry) => {
      const w = mountInspector(site(), {}, { attachTo: document.body });
      await pick(w, entry);
      expect(document.querySelectorAll('[role="dialog"][aria-modal="true"]')).toHaveLength(2);
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      await nextTick();
      expect(w.emitted("close")).toBeUndefined();
      expect(document.querySelectorAll('[role="dialog"][aria-modal="true"]')).toHaveLength(1);
    },
  );

  it("leaves Escape to an open menu or New group popover", async () => {
    const w = mountInspector();
    const dropdown = w.findComponent('[data-testid="dropdown"]') as VueWrapper;
    dropdown.vm.$emit("update:open", true);
    await nextTick();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.emitted("close")).toBeUndefined();
    dropdown.vm.$emit("update:open", false);
    await nextTick();

    const popover = w.findComponent('[data-testid="popover-root"]') as VueWrapper;
    popover.vm.$emit("update:open", true);
    await nextTick();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.emitted("close")).toBeUndefined();
    popover.vm.$emit("update:open", false);
    await nextTick();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.emitted("close")).toHaveLength(1);
  });

  it("ignores Escape in panel mode and while closed", () => {
    const panel = mountInspector(site(), { mode: "panel" });
    const closed = mountInspector(site(), { open: false });
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(panel.emitted("close")).toBeUndefined();
    expect(closed.emitted("close")).toBeUndefined();
    expect(panel.find(".site-inspector-backdrop").exists()).toBe(false);
    expect(panel.find('[role="dialog"]').exists()).toBe(false);
  });
});

describe("SiteInspector WordPress auto-login", () => {
  it("turns auto-login back off and toasts when the admin users can't be read", async () => {
    wordpressAdminUsers.mockRejectedValue(new Error("wp-cli exploded"));
    const on = wpSite({ wp_auto_login: true });
    const w = mountInspector(on);
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith("Couldn't load WordPress admin users", "wp-cli exploded");
    expect(w.emitted("changeWpAutoLogin")).toEqual([[on, false, null, { silent: true }]]);
    expect(w.find('[aria-label="Sign in as"]').exists()).toBe(false);
  });

  it("ignores an admin-user failure that lands after the drawer closes", async () => {
    let fail!: (e: Error) => void;
    wordpressAdminUsers.mockReturnValue(
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
    );
    const w = mountInspector(wpSite({ wp_auto_login: true }));
    await w.setProps({ open: false });
    fail(new Error("wp-cli exploded"));
    await flushPromises();
    expect(toastError).not.toHaveBeenCalled();
    expect(w.emitted("changeWpAutoLogin")).toBeUndefined();
  });

  it("reads no admin users at all while auto-login is off", async () => {
    wordpressAdminUsers.mockRejectedValue(new Error("wp-cli exploded"));
    const w = mountInspector(wpSite({ wp_auto_login: false }));
    await flushPromises();
    expect(wordpressAdminUsers).not.toHaveBeenCalled();
    expect(w.emitted("changeWpAutoLogin")).toBeUndefined();
  });

  it("writes nothing when the fetch the switch triggers fails", async () => {
    wordpressAdminUsers.mockRejectedValue(new Error("still broken"));
    const w = mountInspector(wpSite({ wp_auto_login: false }));
    await flushPromises();
    await w.get('[aria-label="WordPress Auto Admin Login"]').trigger("click");
    await flushPromises();
    expect(wordpressAdminUsers).toHaveBeenCalledOnce();
    expect(toastError).toHaveBeenCalledOnce();
    expect(w.emitted("changeWpAutoLogin")).toBeUndefined();
  });

  it("writes the change once a retried fetch succeeds", async () => {
    wordpressAdminUsers.mockRejectedValueOnce(new Error("transient"));
    const off = wpSite({ wp_auto_login: false });
    const w = mountInspector(off);
    await flushPromises();
    await w.get('[aria-label="WordPress Auto Admin Login"]').trigger("click");
    await flushPromises();
    expect(w.emitted("changeWpAutoLogin")).toBeUndefined();
    wordpressAdminUsers.mockResolvedValue([{ login: "editor", display_name: "Editor" }]);
    await w.get('[aria-label="WordPress Auto Admin Login"]').trigger("click");
    await flushPromises();
    expect(w.emitted("changeWpAutoLogin")).toEqual([[off, true, null]]);
  });

  it("shows the user picker only once the admin list has loaded", async () => {
    const w = mountInspector(wpSite({ wp_auto_login: true }));
    expect(w.find('[aria-label="Sign in as"]').exists()).toBe(false);
    await flushPromises();
    expect(w.find('[aria-label="Sign in as"]').exists()).toBe(true);
  });

  it("toggles auto-login and changes the signed-in user", async () => {
    wordpressAdminUsers.mockResolvedValue([{ login: "editor", display_name: "Editor" }]);
    const off = wpSite({ wp_auto_login: false });
    const w = mountInspector(off);
    await w.get('[aria-label="WordPress Auto Admin Login"]').trigger("click");
    await flushPromises();
    expect(w.emitted("changeWpAutoLogin")).toEqual([[off, true, null]]);
    const on = wpSite({ wp_auto_login: true });
    await w.setProps({ site: on });
    await flushPromises();
    await w.get('[aria-label="Sign in as"]').setValue("editor");
    expect(w.emitted("changeWpAutoLogin")?.[1]).toEqual([on, true, "editor"]);
  });

  it("hides WordPress-only controls on a non-WordPress site", () => {
    const w = mountInspector();
    expect(w.find('[aria-label="WordPress Auto Admin Login"]').exists()).toBe(false);
    expect(wordpressAdminUsers).not.toHaveBeenCalled();
  });
});
