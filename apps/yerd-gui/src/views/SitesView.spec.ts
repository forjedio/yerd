import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const openUrl = vi.fn();
const revealItemInDir = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => openUrl(...args),
  revealItemInDir: (...args: unknown[]) => revealItemInDir(...args),
  openPath: vi.fn(),
}));

const dialogOpen = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => dialogOpen(...args),
  save: vi.fn(),
}));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
}));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = { report: ref(null), connected: ref(true), refresh: vi.fn() };
  return { useDaemon: () => state };
});

import SitesView from "./SitesView.vue";
import NewSiteDialog from "@/components/site-create/NewSiteDialog.vue";
import LinkFolderDialog from "@/components/sites/LinkFolderDialog.vue";
import SiteInspector from "@/components/sites/SiteInspector.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { resetResourceCache } from "@/composables/useResource";
import { resetSitesGroupState } from "@/lib/sitesGroupState";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ca: { path: "", fingerprint: "", trusted_system: true },
    default_php: "8.4",
    php: [{ version: "8.3" }, { version: "8.4" }],
    ...overrides,
  } as StatusReport;
}

function site(name: string, overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.4",
    secure: false,
    kind: "parked",
    ...overrides,
  };
}

function wpSite(name: string, overrides: Partial<SiteEntry> = {}): SiteEntry {
  return site(name, {
    kind: "linked",
    document_root: `/code/${name}`,
    is_wordpress: true,
    framework: "wordpress",
    wp_auto_login: true,
    ...overrides,
  });
}

type Handler = (args?: Record<string, unknown>) => unknown;

interface World {
  sites: SiteEntry[];
  parked?: string[];
  groups?: GroupsState;
  routes?: Record<string, Handler>;
}

function serve(world: World): void {
  const routes: Record<string, Handler> = {
    list_sites: () => ({ type: "sites", sites: world.sites }),
    list_parked: () => ({ type: "parked", paths: world.parked ?? [] }),
    list_groups: () => ({ type: "groups", ...(world.groups ?? { order: [], members: {} }) }),
    node_sites: () => ({ type: "node_sites", sites: [] }),
    node_site: () => ({ type: "node_site" }),
    list_node: () => ({ type: "node_versions", installed: [] }),
    host_platform: () => "macos",
    get_installed_ides: () => [],
    get_preferred_ide: () => null,
    get_site_ide_overrides: () => ({}),
    ...world.routes,
  };
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    const handler = routes[cmd];
    return handler
      ? Promise.resolve(handler(args))
      : Promise.reject(new Error(`unexpected invoke ${cmd}`));
  });
}

function calls(cmd: string): unknown[] {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);
}

let wideMode = false;

function setWide(wide: boolean): void {
  wideMode = wide;
  globalThis.matchMedia = vi.fn(() => ({
    matches: wide,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof globalThis.matchMedia;
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountSites() {
  const wrapper = mount(SitesView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: !wideMode,
        DialogPortal: Pass,
        AlertDialogPortal: Pass,
        ComboboxPortal: Pass,
      },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type Sites = Awaited<ReturnType<typeof mountSites>>;

function row(w: Sites, name: string) {
  return w.get(`[data-site="${name}"]`);
}

function rowNames(w: Sites): string[] {
  return w.findAll("[data-site]").map((r) => r.attributes("data-site") ?? "");
}

function button(w: Sites, text: string) {
  const b = w.findAll("button").find((x) => x.text().includes(text));
  if (!b) throw new Error(`no "${text}" button`);
  return b;
}

async function filter(w: Sites, query: string) {
  await w.get('input[aria-label="Filter sites"]').setValue(query);
  await flushPromises();
}

async function key(w: Sites, init: KeyboardEventInit) {
  await w.get('[data-testid="sites-list"]').trigger("keydown", init);
  await flushPromises();
}

beforeEach(() => {
  invokeMock.mockReset();
  openUrl.mockReset().mockResolvedValue(undefined);
  revealItemInDir.mockReset().mockResolvedValue(undefined);
  dialogOpen.mockReset();
  push.mockReset();
  resetResourceCache();
  resetSitesGroupState();
  setWide(false);
  useDaemon().report.value = report();
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});

afterEach(() => {
  mounted.splice(0).forEach((w) => w.unmount());
  document.body.innerHTML = "";
});

describe("SitesView list", () => {
  const world = (): World => ({
    sites: [
      site("alpha", { framework: "laravel", secure: true }),
      site("beta", { php: "8.3" }),
      site("lnk", { kind: "linked", document_root: "/elsewhere/lnk" }),
      site("member"),
    ],
    parked: ["/srv"],
    groups: { order: ["Work"], members: { member: "Work" } },
  });

  it("orders the sections group, folder, Linked, with their counts", async () => {
    serve(world());
    const w = await mountSites();
    const sections = w.findAll("[data-section]");
    expect(sections.map((s) => s.attributes("data-section"))).toEqual([
      "Work",
      "dir:/srv",
      ":linked",
    ]);
    expect(sections[1]?.text()).toContain("/srv");
    expect(sections[1]?.find("button").text()).toMatch(/2$/);
    expect(rowNames(w)).toEqual(["member", "alpha", "beta", "lnk"]);
  });

  it("counts the chips and filters by one", async () => {
    serve(world());
    const w = await mountSites();
    const chips = w.findAll("[data-chip]").map((c) => c.text().replace(/\s+/g, " "));
    expect(chips).toEqual(["All 4", "HTTPS 1", "Linked 1", "Laravel 1", "Not on 8.4 1"]);
    await w.get('[data-chip="https"]').trigger("click");
    expect(rowNames(w)).toEqual(["alpha"]);
    expect(w.find('[data-section="Work"]').exists()).toBe(false);
  });

  it("filters by path", async () => {
    serve(world());
    const w = await mountSites();
    await filter(w, "/elsewhere");
    expect(rowNames(w)).toEqual(["lnk"]);
  });

  it("drops empty groups and folders under a chip alone", async () => {
    serve({ ...world(), groups: { order: ["Work", "Empty"], members: { member: "Work" } } });
    const w = await mountSites();
    expect(w.find('[data-section="Empty"]').exists()).toBe(true);
    await w.get('[data-chip="linked"]').trigger("click");
    expect(w.findAll("[data-section]").map((s) => s.attributes("data-section"))).toEqual([":linked"]);
  });

  it("hides group controls while filtering", async () => {
    serve(world());
    const w = await mountSites();
    expect(w.find('[aria-label="Move Work up"]').exists()).toBe(true);
    await filter(w, "mem");
    expect(w.find('[aria-label="Move Work up"]').exists()).toBe(false);
  });

  it("cuts a long folder and expands it", async () => {
    const many = Array.from({ length: 12 }, (_, i) => site(`s${String(i).padStart(2, "0")}`));
    serve({ sites: many, parked: ["/srv"] });
    const w = await mountSites();
    expect(rowNames(w)).toHaveLength(10);
    await button(w, "+ 2 more in /srv").trigger("click");
    expect(rowNames(w)).toHaveLength(12);
  });

  it("remembers a collapsed section", async () => {
    serve(world());
    const w = await mountSites();
    await w.get('[data-section="dir:/srv"] button').trigger("click");
    expect(rowNames(w)).not.toContain("alpha");
    w.unmount();
    mounted.splice(0);
    const again = await mountSites();
    expect(rowNames(again)).not.toContain("alpha");
    expect(rowNames(again)).toContain("lnk");
  });

  it("shows the Node column from node_sites in the table", async () => {
    serve({
      ...world(),
      routes: {
        node_sites: () => ({
          type: "node_sites",
          sites: [{ site: "alpha", effective: "22.12.0", source: "default" }],
        }),
      },
    });
    const w = await mountSites();
    expect(w.find('[data-layout="compact"]').exists()).toBe(true);
    expect(row(w, "alpha").get('[data-testid="row-node"]').text()).toBe("22.12.0");
    expect(row(w, "beta").get('[data-testid="row-node"]').text()).toBe("—");
  });

  it("lays out list and inspector side by side when wide", async () => {
    setWide(true);
    serve(world());
    const w = await mountSites();
    expect(w.find('[data-layout="wide"]').exists()).toBe(true);
    expect(w.find('[data-testid="inspector-column"]').text()).toContain("member.test");
    expect(row(w, "member").attributes("aria-selected")).toBe("true");
    expect(w.find('[data-testid="row-node"]').exists()).toBe(false);
  });
});

describe("SitesView folder sections", () => {
  const world = (): World => ({ sites: [site("a"), site("b")], parked: ["/srv"] });

  it("reveals a parked folder", async () => {
    serve(world());
    const w = await mountSites();
    await w.get('[aria-label="Reveal /srv"]').trigger("click");
    await flushPromises();
    expect(revealItemInDir).toHaveBeenCalledWith("/srv");
  });

  it("unparks a folder after confirming, and not on Cancel", async () => {
    serve({ ...world(), routes: { unpark: () => ({ type: "ok" }) } });
    const w = await mountSites();
    await w.get('[aria-label="Unpark /srv"]').trigger("click");
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Unpark /srv?");
    expect(dialog.text()).toContain("2 sites in this folder stop being served.");
    await button(w, "Cancel").trigger("click");
    await flushPromises();
    expect(calls("unpark")).toEqual([]);

    await w.get('[aria-label="Unpark /srv"]').trigger("click");
    await flushPromises();
    await button(w, "Unpark 2 sites").trigger("click");
    await flushPromises();
    expect(calls("unpark")).toEqual([{ path: "/srv" }]);
  });
});

describe("SitesView keyboard", () => {
  it("moves the selection and opens, opens a terminal and copies the URL", async () => {
    setWide(true);
    serve({
      sites: [site("alpha"), site("beta", { secure: true })],
      parked: ["/srv"],
      routes: { open_terminal: () => undefined },
    });
    const w = await mountSites();
    expect(row(w, "alpha").attributes("aria-selected")).toBe("true");
    await key(w, { key: "ArrowDown" });
    expect(row(w, "beta").attributes("aria-selected")).toBe("true");
    await key(w, { key: "ArrowUp" });
    await key(w, { key: "ArrowDown" });
    await key(w, { key: "Enter" });
    expect(openUrl).toHaveBeenCalledWith("https://beta.test");
    await key(w, { key: "t", metaKey: true });
    expect(calls("open_terminal")).toEqual([{ path: "/srv/beta" }]);
    await key(w, { key: "c", metaKey: true });
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("https://beta.test");
  });
});

describe("SitesView selection", () => {
  it("keeps inspecting a wide selection a filter hides, until the site is gone", async () => {
    setWide(true);
    const world: World = { sites: [site("alpha"), site("beta")], parked: ["/srv"] };
    serve(world);
    const w = await mountSites();
    expect(w.get('[data-testid="inspector-column"]').text()).toContain("alpha.test");
    await filter(w, "beta");
    expect(rowNames(w)).toEqual(["beta"]);
    expect(w.get('[data-testid="inspector-column"]').text()).toContain("alpha.test");
    world.sites = [site("beta")];
    w.findComponent(SiteInspector).vm.$emit("domainsChanged");
    await flushPromises();
    expect(w.get('[data-testid="inspector-column"]').text()).toContain("beta.test");
  });

  it("ignores list shortcuts for a compact selection a filter hides", async () => {
    serve({ sites: [site("alpha"), site("beta")], parked: ["/srv"] });
    const w = await mountSites();
    await row(w, "alpha").trigger("click");
    await flushPromises();
    await w.get('[aria-label="Close site details"]').trigger("click");
    await filter(w, "beta");
    await key(w, { key: "Enter" });
    await key(w, { key: "c", metaKey: true });
    expect(openUrl).not.toHaveBeenCalled();
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(w.get('[data-testid="sites-list"]').attributes("aria-activedescendant")).toBeUndefined();
  });

  it.each([" ", "ArrowRight"])("opens the compact drawer for the selected row on %j", async (k) => {
    serve({ sites: [site("alpha"), site("beta")], parked: ["/srv"] });
    const w = await mountSites();
    expect(document.body.querySelector('[data-testid="site-inspector"]')).toBeNull();
    await key(w, { key: "ArrowDown" });
    await key(w, { key: "ArrowDown" });
    await key(w, { key: k });
    expect(document.body.querySelector('[data-testid="site-inspector"]')?.textContent).toContain(
      "beta.test",
    );
    expect(w.get('[data-testid="sites-hints"]').text()).toContain("Space details");
  });

  it("passes every site name to New site so a taken name is refused", async () => {
    serve({ sites: [site("alpha"), site("lnk", { kind: "linked" })], parked: ["/srv"] });
    const w = await mountSites();
    expect(w.findComponent(NewSiteDialog).props("existingNames")).toEqual(["alpha", "lnk"]);
  });
});

describe("SitesView states", () => {
  it("offers to park, create or link when there is nothing yet", async () => {
    serve({ sites: [], routes: { park: () => ({ type: "ok" }) } });
    dialogOpen.mockResolvedValue("/srv/new");
    const w = await mountSites();
    expect(w.text()).toContain("Serve your first site");
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(calls("park")).toEqual([{ path: "/srv/new" }]);
  });

  it("offers Create from a query with no match, and Clear filter resets", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"] });
    const w = await mountSites();
    await w.get('[data-chip="all"]').trigger("click");
    await filter(w, "Invo");
    const empty = w.get('[data-testid="sites-no-match"]');
    expect(empty.text()).toContain("Nothing matches “Invo”");
    await button(w, "Create invo.test").trigger("click");
    await flushPromises();
    expect((w.get("#ns-name").element as HTMLInputElement).value).toBe("invo");
    await button(w, "Clear filter").trigger("click");
    expect(rowNames(w)).toEqual(["alpha"]);
    expect(w.get('[data-chip="all"]').attributes("aria-pressed")).toBe("true");
  });

  it.each([
    [
      "ports",
      {
        http: { requested: 80, bound: 8080, fell_back: true },
        https: { requested: 443, bound: 8443, fell_back: true },
        foreign_web_listener: true,
      },
      "Run Doctor",
    ],
    ["ca", { ca: { path: "", fingerprint: "", trusted_system: false } }, "Trust now"],
  ] as const)("shows the %s strip and routes to Doctor", async (key, overrides, action) => {
    useDaemon().report.value = report(overrides as Partial<StatusReport>);
    serve({ sites: [site("alpha")], parked: ["/srv"] });
    const w = await mountSites();
    const strip = w.get(`[data-warning="${key}"]`);
    await strip.findAll("button").find((b) => b.text().includes(action))?.trigger("click");
    expect(push).toHaveBeenCalledWith("/doctor");
  });

  it("shows no strip for a healthy report", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"] });
    const w = await mountSites();
    expect(w.find("[data-warning]").exists()).toBe(false);
  });
});

describe("SitesView inspector wiring", () => {
  async function openDrawer(w: Sites, name: string) {
    await row(w, name).trigger("click");
    await flushPromises();
    return w.findComponent(SiteInspector);
  }

  it("removes a linked site after confirming", async () => {
    serve({
      sites: [site("lnk", { kind: "linked", document_root: "/code/lnk" })],
      routes: { unlink: () => ({ type: "ok" }) },
    });
    const w = await mountSites();
    await openDrawer(w, "lnk");
    await w.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Remove lnk.test?");
    expect(dialog.text()).toContain("/code/lnk is not deleted.");
    await dialog.findAll("button").find((b) => b.text() === "Remove site")?.trigger("click");
    await flushPromises();
    expect(calls("unlink")).toEqual([{ name: "lnk" }]);
  });

  function toastFor(title: string) {
    return useToast().toasts.value.find((t) => t.title === title);
  }

  const providersRoute = (provider: string): Handler => () => ({
    type: "tunnel_providers",
    providers: [],
    default_provider: provider,
  });

  it("reports an old daemon that shared through Cloudflare instead of the default provider", async () => {
    serve({
      sites: [site("alpha")],
      parked: ["/srv"],
      routes: {
        list_tunnel_providers: providersRoute("ngrok"),
        tunnel_status: () => ({ type: "tunnels", tunnels: [], cloudflared: { installed: true, logged_in: false } }),
        start_quick_tunnel: () => ({
          type: "tunnels",
          tunnels: [{ site: "alpha", kind: "quick", state: "running", url: "https://a.trycloudflare.com" }],
        }),
        stop_tunnel: () => ({ type: "tunnels", tunnels: [] }),
      },
    });
    const w = await mountSites();
    const inspector = await openDrawer(w, "alpha");
    inspector.vm.$emit("share", site("alpha"));
    await flushPromises();
    expect(calls("stop_tunnel")).toEqual([{ site: "alpha", provider: "cloudflare" }]);
    expect(toastFor("Couldn't share alpha")?.detail).toBe(
      "The running Yerd daemon doesn't support ngrok sharing yet - restart Yerd and try again",
    );
    expect(toastFor("Sharing alpha")).toBeUndefined();
    expect(toastFor("Couldn't stop the Cloudflare share of alpha")).toBeUndefined();
  });

  it("warns that the Cloudflare share may still be live when stopping it fails", async () => {
    serve({
      sites: [site("alpha")],
      parked: ["/srv"],
      routes: {
        list_tunnel_providers: providersRoute("ngrok"),
        tunnel_status: () => ({ type: "tunnels", tunnels: [], cloudflared: { installed: true, logged_in: false } }),
        start_quick_tunnel: () => ({
          type: "tunnels",
          tunnels: [{ site: "alpha", kind: "quick", state: "running", url: "https://a.trycloudflare.com" }],
        }),
        stop_tunnel: () => Promise.reject(new Error("daemon busy")),
      },
    });
    const w = await mountSites();
    const inspector = await openDrawer(w, "alpha");
    inspector.vm.$emit("share", site("alpha"));
    await flushPromises();
    expect(calls("stop_tunnel")).toEqual([{ site: "alpha", provider: "cloudflare" }]);
    expect(toastFor("Couldn't share alpha")).toBeDefined();
    expect(toastFor("Couldn't stop the Cloudflare share of alpha")?.detail).toBe(
      "alpha may still be public through its Cloudflare quick link - stop it on the Share page",
    );
  });

  it("shares through the default provider when the daemon honours it", async () => {
    serve({
      sites: [site("alpha")],
      parked: ["/srv"],
      routes: {
        list_tunnel_providers: providersRoute("ngrok"),
        tunnel_status: () => ({ type: "tunnels", tunnels: [], cloudflared: { installed: true, logged_in: false } }),
        start_quick_tunnel: () => ({
          type: "tunnels",
          tunnels: [{ site: "alpha", kind: "quick", state: "running", url: "https://a.ngrok.app", provider: "ngrok" }],
        }),
      },
    });
    const w = await mountSites();
    const inspector = await openDrawer(w, "alpha");
    inspector.vm.$emit("share", site("alpha"));
    await flushPromises();
    expect(calls("stop_tunnel")).toEqual([]);
    expect(toastFor("Sharing alpha")?.detail).toContain("https://a.ngrok.app");
  });

  it("stars a site", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"], routes: { set_site_favourite: () => ({ type: "ok" }) } });
    const w = await mountSites();
    await openDrawer(w, "alpha");
    await w.get('[aria-label="Star alpha"]').trigger("click");
    await flushPromises();
    expect(calls("set_site_favourite")).toEqual([{ site: "alpha", favourite: true }]);
  });

  it("stars a site from its row star without opening it", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"], routes: { set_site_favourite: () => ({ type: "ok" }) } });
    const w = await mountSites();
    await row(w, "alpha").get('[data-testid="row-star"]').trigger("click");
    await flushPromises();
    expect(calls("set_site_favourite")).toEqual([{ site: "alpha", favourite: true }]);
    expect(document.body.querySelector('[data-testid="site-inspector"]')).toBeNull();
  });

  it("does not paint the selected row in the compact table outside keyboard focus", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"] });
    const w = await mountSites();
    await openDrawer(w, "alpha");
    const cls = row(w, "alpha").classes();
    expect(cls).not.toContain("bg-brand/25");
    expect(cls).toContain("group-focus-visible/list:bg-brand/25");
  });

  it("refetches the Node column when the inspector changes a site's Node", async () => {
    serve({ sites: [site("alpha")], parked: ["/srv"] });
    const w = await mountSites();
    const inspector = await openDrawer(w, "alpha");
    const before = calls("node_sites").length;
    inspector.vm.$emit("nodeChanged");
    await flushPromises();
    expect(calls("node_sites").length).toBe(before + 1);
  });

  it("creates a group and moves the site into it", async () => {
    serve({
      sites: [site("alpha")],
      parked: ["/srv"],
      routes: { create_group: () => ({ type: "ok" }), set_site_group: () => ({ type: "ok" }) },
    });
    const w = await mountSites();
    const inspector = await openDrawer(w, "alpha");
    inspector.vm.$emit("createGroupAndMove", site("alpha"), "Clients");
    await flushPromises();
    expect(invokeMock.mock.calls.map((c) => c[0]).filter((c) => c.includes("group"))).toEqual(
      expect.arrayContaining(["create_group", "set_site_group"]),
    );
    expect(calls("create_group")).toEqual([{ name: "Clients" }]);
    expect(calls("set_site_group")).toEqual([{ site: "alpha", group: "Clients" }]);
  });

  it("creates an empty group from the toolbar and rejects a duplicate name", async () => {
    serve({
      sites: [site("alpha")],
      parked: ["/srv"],
      groups: { order: ["Clients"], members: {} },
      routes: { create_group: () => ({ type: "ok" }) },
    });
    const w = await mountSites();
    const newGroup = w.findAll("button").find((b) => b.text().includes("New group"));
    await newGroup?.trigger("click");
    await flushPromises();
    const input = () => w.get("#newgroupname");
    const create = () => w.findAll("button").find((b) => b.text() === "Create");
    await input().setValue("clients");
    await flushPromises();
    expect(create()?.attributes("disabled")).toBeDefined();
    await input().setValue("Personal");
    await flushPromises();
    expect(create()?.attributes("disabled")).toBeUndefined();
    await create()?.trigger("click");
    await flushPromises();
    expect(calls("create_group")).toEqual([{ name: "Personal" }]);
  });
});

describe("SitesView link", () => {
  function linkWorld(after: Partial<SiteEntry> = {}) {
    const linked: SiteEntry[] = [];
    serve({
      sites: linked,
      parked: ["/srv"],
      routes: {
        link: (args) => {
          linked.push(site(String(args?.name), { kind: "linked", document_root: String(args?.path), ...after }));
          return { type: "ok" };
        },
        set_php: () => ({ type: "ok" }),
        set_secure: () => ({ type: "ok" }),
      },
    });
  }

  function writes(): string[] {
    return invokeMock.mock.calls
      .map((c) => c[0] as string)
      .filter((c) => ["link", "set_php", "set_secure"].includes(c));
  }

  it("only links when the dialog's choices match what the daemon assigned", async () => {
    linkWorld();
    const w = await mountSites();
    w.findComponent(LinkFolderDialog).vm.$emit("link", {
      name: "app",
      path: "/code/app",
      php: "8.4",
      secure: false,
    });
    await flushPromises();
    expect(writes()).toEqual(["link"]);
  });

  it("links, then sets PHP and HTTPS in order when they differ", async () => {
    linkWorld();
    const w = await mountSites();
    w.findComponent(LinkFolderDialog).vm.$emit("link", {
      name: "app",
      path: "/code/app",
      php: "8.3",
      secure: true,
    });
    await flushPromises();
    expect(writes()).toEqual(["link", "set_php", "set_secure"]);
    expect(calls("set_php")).toEqual([{ name: "app", version: "8.3" }]);
    expect(calls("set_secure")).toEqual([{ name: "app", secure: true }]);
  });

  it("only links when an unapplied .yerd.toml leaves PHP and HTTPS at the defaults", async () => {
    linkWorld({ php: "8.4", secure: true });
    const w = await mountSites();
    w.findComponent(LinkFolderDialog).vm.$emit("link", {
      name: "app",
      path: "/code/app",
      php: null,
      secure: null,
      applyProjectFile: false,
    });
    await flushPromises();
    expect(writes()).toEqual(["link"]);
  });
});

describe("SitesView .yerd.toml", () => {
  function applyWorld(log: string[], state = "succeeded", error?: string) {
    serve({
      sites: [site("acme", { kind: "linked", document_root: "/code/acme" })],
      routes: {
        apply_project_config: () => ({ type: "job_started", job_id: "j1" }),
        job_status: () => ({
          type: "job_progress",
          state,
          phase: "Done",
          log,
          next_cursor: log.length,
          error,
        }),
        export_project_config: () => ({
          type: "project_config",
          file: "/code/acme/.yerd.toml",
          exists: true,
          site: "acme",
          name: "acme",
          contents: "",
        }),
        link: () => ({ type: "ok" }),
        set_php: () => ({ type: "ok" }),
        set_secure: () => ({ type: "ok" }),
      },
    });
  }

  beforeEach(() => {
    for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  });

  function toasts(): string {
    return useToast()
      .toasts.value.map((t) => `${t.title} ${t.detail ?? ""}`)
      .join("\n");
  }

  it("applies the file through the daemon instead of linking from the dialog", async () => {
    applyWorld(["applied: linked acme", "done: 1 applied, 0 unchanged, 0 warning(s), 0 failed"]);
    const w = await mountSites();
    w.findComponent(LinkFolderDialog).vm.$emit("link", {
      name: "acme",
      path: "/code/acme",
      php: "8.3",
      secure: true,
      applyProjectFile: true,
    });
    await flushPromises();
    expect(calls("apply_project_config")).toEqual([{ path: "/code/acme" }]);
    expect(calls("link")).toEqual([]);
    expect(calls("set_php")).toEqual([]);
    expect(calls("set_secure")).toEqual([]);
    expect(toasts()).toContain("Applied .yerd.toml for acme");
  });

  it("selects the site the file linked at the folder, not the dialog's name", async () => {
    setWide(true);
    serve({
      sites: [
        site("acme", { kind: "linked", document_root: "/code/checkout" }),
        site("checkout", { document_root: "/code/other" }),
      ],
      routes: {
        apply_project_config: () => ({ type: "job_started", job_id: "j1" }),
        job_status: () => ({
          type: "job_progress",
          state: "succeeded",
          phase: "Done",
          log: ["applied: linked acme"],
          next_cursor: 1,
        }),
      },
    });
    const w = await mountSites();
    w.findComponent(LinkFolderDialog).vm.$emit("link", {
      name: "checkout",
      path: "/code/checkout/",
      php: "8.3",
      secure: false,
      applyProjectFile: true,
    });
    await flushPromises();
    expect(row(w, "acme").attributes("aria-selected")).toBe("true");
    expect(row(w, "checkout").attributes("aria-selected")).not.toBe("true");
  });

  it("counts the job's warnings in the toast", async () => {
    applyWorld([
      "warning: domain shared.test: owned elsewhere; skipped",
      "warning: extension imagick is not available in PHP 8.4",
      "done: 0 applied, 3 unchanged, 2 warning(s), 0 failed",
    ]);
    const w = await mountSites();
    w.findComponent(SiteInspector).vm.$emit("applyProjectFile", site("acme", { document_root: "/code/acme" }));
    await flushPromises();
    expect(calls("apply_project_config")).toEqual([{ path: "/code/acme" }]);
    expect(toasts()).toContain("with 2 warnings");
    expect(toasts()).toContain("extension imagick is not available in PHP 8.4");
  });

  it("reports a failed apply with its failed items", async () => {
    applyWorld(
      ["failed: install PHP 8.5: offline", "done: 0 applied, 0 unchanged, 0 warning(s), 1 failed"],
      "failed",
      "1 item(s) could not be applied",
    );
    const w = await mountSites();
    w.findComponent(SiteInspector).vm.$emit("applyProjectFile", site("acme", { document_root: "/code/acme" }));
    await flushPromises();
    expect(toasts()).toContain("Couldn't apply .yerd.toml");
    expect(toasts()).toContain("install PHP 8.5: offline");
  });

  it("exports a site and toasts the file path", async () => {
    applyWorld([]);
    const w = await mountSites();
    w.findComponent(SiteInspector).vm.$emit("exportProjectFile", site("acme"));
    await flushPromises();
    expect(calls("export_project_config")).toEqual([{ site: "acme" }]);
    expect(toasts()).toContain("Exported acme to .yerd.toml");
  });
});

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function usersEnvelope(logins: string[]) {
  return {
    type: "wordpress_admin_users",
    users: logins.map((login) => ({ login, display_name: login })),
  };
}

describe("SitesView WordPress auto-login controls", () => {
  beforeEach(() => {
    setWide(true);
  });

  it("loads the admin-user picker for the selected site", async () => {
    serve({
      sites: [wpSite("alpha")],
      routes: { wordpress_admin_users: () => usersEnvelope(["editor"]) },
    });
    const w = await mountSites();
    const select = w.get("#site-wp-admin-user");
    expect(select.attributes("disabled")).toBeUndefined();
    expect(select.findAll("option").map((o) => o.text())).toEqual([
      "Earliest admin (default)",
      "editor",
    ]);
  });

  it("does not let a slow response for a previously selected site overwrite the current one", async () => {
    const alphaUsers = deferred<unknown>();
    const betaUsers = deferred<unknown>();
    serve({
      sites: [wpSite("alpha"), wpSite("beta")],
      routes: {
        wordpress_admin_users: (args) =>
          args?.site === "alpha" ? alphaUsers.promise : betaUsers.promise,
      },
    });
    const w = await mountSites();
    await row(w, "beta").trigger("click");
    await flushPromises();
    betaUsers.resolve(usersEnvelope(["beta-editor"]));
    await flushPromises();
    alphaUsers.resolve(usersEnvelope(["alpha-editor"]));
    await flushPromises();
    expect(w.get("#site-wp-admin-user").findAll("option").map((o) => o.text())).toEqual([
      "Earliest admin (default)",
      "beta-editor",
    ]);
  });

  it("withholds the picker until the admin list arrives, then selects the configured admin", async () => {
    const pending = deferred<unknown>();
    serve({
      sites: [wpSite("alpha", { wp_auto_login_user: "editor" })],
      routes: { wordpress_admin_users: () => pending.promise },
    });
    const w = await mountSites();
    expect(w.find("#site-wp-admin-user").exists()).toBe(false);
    pending.resolve(usersEnvelope(["editor"]));
    await flushPromises();
    await nextTick();
    expect(w.get<HTMLSelectElement>("#site-wp-admin-user").element.value).toBe("editor");
  });

  it("hides the picker but writes nothing when the fetch for an auto-selected site fails", async () => {
    serve({
      sites: [wpSite("alpha", { wp_auto_login_user: "editor" })],
      routes: {
        wordpress_admin_users: () => Promise.reject(new Error("boom")),
        set_wordpress_auto_login: () => ({ type: "ok" }),
      },
    });
    const w = await mountSites();
    await flushPromises();
    expect(w.find("#site-wp-admin-user").exists()).toBe(false);
    expect(w.get('[data-testid="wp-admin-users-error"]').text()).toContain("boom");
    expect(calls("set_wordpress_auto_login")).toEqual([]);
  });
});
