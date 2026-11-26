import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter, type Router } from "vue-router";

import type {
  Contributor,
  ReleaseNotesEntry,
  ServiceStatus,
  SiteEntry,
  StatusReport,
} from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  sitesAndParked: vi.fn(),
  daemonVersionConflict: vi.fn(),
  releaseNotes: vi.fn(),
  contributors: vi.fn(),
  stopDaemon: vi.fn(),
  restartDaemon: vi.fn(),
  openInBrowser: vi.fn(),
  pickDirectory: vi.fn(),
  park: vi.fn(),
  setSiteFavourite: vi.fn(),
  tunnelStatus: vi.fn(),
  startService: vi.fn(),
  stopService: vi.fn(),
  restartService: vi.fn(),
  checkUpdates: vi.fn(),
  refresh: vi.fn(),
}));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = {
    connected: ref<boolean | null>(null),
    report: ref<StatusReport | null>(null),
    lastError: ref<{ message: string } | null>(null),
    refresh: mocks.refresh,
  };
  return { useDaemon: () => state, daemonState: state };
});

vi.mock("@/composables/useDaemonStart", async () => {
  const { ref } = await import("vue");
  const state = {
    starting: ref(false),
    activeLabel: ref<string | null>(null),
    pendingApproval: ref(false),
    diagnostics: ref(null),
    start: vi.fn(),
  };
  return { useDaemonStart: () => state };
});

vi.mock("@/ipc/client", () => ({
  sitesAndParked: mocks.sitesAndParked,
  daemonVersionConflict: mocks.daemonVersionConflict,
  releaseNotes: mocks.releaseNotes,
  contributors: mocks.contributors,
  stopDaemon: mocks.stopDaemon,
  restartDaemon: mocks.restartDaemon,
  openInBrowser: mocks.openInBrowser,
  pickDirectory: mocks.pickDirectory,
  park: mocks.park,
  setSiteFavourite: mocks.setSiteFavourite,
  tunnelStatus: mocks.tunnelStatus,
  startService: mocks.startService,
  stopService: mocks.stopService,
  restartService: mocks.restartService,
  checkUpdates: mocks.checkUpdates,
  cachedUpdateStatus: vi.fn(async () => null),
  applyUpdate: vi.fn(),
  setUpdateChannel: vi.fn(),
  homeDirectory: vi.fn(async () => "/Users/me"),
  openLoginItems: vi.fn(async () => {}),
  hostPlatform: vi.fn(async () => "macos"),
  status: vi.fn(),
  setupState: vi.fn(),
  markOnboarded: vi.fn(),
  IpcError: class IpcError extends Error {},
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "main" }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

import * as daemonModule from "@/composables/useDaemon";
import { useOnboarding } from "@/composables/useOnboarding";
import { resetResourceCache } from "@/composables/useResource";
import { servicesIntent } from "@/lib/shortcuts/servicesIntent";
import { sitesIntent, sitesSelectIntent } from "@/lib/shortcuts/sitesIntent";

import OverviewView from "./OverviewView.vue";

const daemon = (
  daemonModule as unknown as {
    daemonState: {
      connected: { value: boolean | null };
      report: { value: StatusReport | null };
      lastError: { value: { message: string } | null };
    };
  }
).daemonState;

function makeReport(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    daemon_pid: 55921,
    uptime_secs: 660,
    daemon_version: "2.1.0-rc.1",
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    port_redirect: true,
    ca: { trusted_system: true },
    resolver_installed: true,
    default_php: "8.5",
    php: [{ version: "8.5" }],
    sites: { parked: 60, linked: 2, secured: 0 },
    services: [service("mysql", "MySQL", "running", 3306), service("redis", "Redis", "running", 6379)],
    ...overrides,
  } as unknown as StatusReport;
}

function service(id: string, name: string, state: string, port: number, installed = true): ServiceStatus {
  return {
    service: id,
    display_name: name,
    installed_versions: installed ? ["8.4"] : [],
    selected_version: installed ? "8.4" : null,
    state,
    pid: null,
    listen: null,
    port,
    enabled: true,
    supports_databases: false,
  } as ServiceStatus;
}

function site(name: string, overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name,
    document_root: `/Users/me/Herd/${name}`,
    php: "8.5",
    secure: true,
    kind: "parked",
    ...overrides,
  } as SiteEntry;
}

function sitesList(n: number, extra: SiteEntry[] = []): SiteEntry[] {
  return [...extra, ...Array.from({ length: n - extra.length }, (_, i) => site(`site${i}`))];
}

function release(tag: string, overrides: Partial<ReleaseNotesEntry> = {}): ReleaseNotesEntry {
  return {
    version: tag.slice(1),
    tag,
    published_at: "2026-08-15T10:00:00Z",
    prerelease: false,
    summary: `Summary of ${tag}`,
    features: [{ title: `Feature in ${tag}` }],
    fixes: [{ title: `Fix in ${tag}` }],
    others: [],
    ...overrides,
  };
}

function people(n: number): Contributor[] {
  return Array.from({ length: n }, (_, i) => ({
    login: `user${i}`,
    avatar_url: `https://avatars.example/${i}`,
    commits: n - i,
  }));
}

const NO_GROUPS = { order: [], members: {} };

let wrapper: VueWrapper | null = null;
let router: Router;

async function mountView(): Promise<VueWrapper> {
  router = createRouter({
    history: createMemoryHistory(),
    routes: ["/overview", "/sites", "/services", "/doctor", "/general", "/php"].map((path) => ({
      path,
      component: { template: "<div />" },
    })),
  });
  await router.push("/overview");
  wrapper = mount(OverviewView, {
    attachTo: document.body,
    global: { plugins: [router] },
  });
  await flushPromises();
  return wrapper;
}

function buttonByText(w: VueWrapper, text: string) {
  const found = w.findAll("button").find((b) => b.text().trim() === text);
  if (!found) throw new Error(`no button "${text}"`);
  return found;
}

function bodyButton(text: string): HTMLElement {
  const found = Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find(
    (b) => b.textContent?.trim() === text,
  );
  if (!(found instanceof HTMLElement)) throw new Error(`no dialog button "${text}"`);
  return found;
}

beforeEach(() => {
  vi.clearAllMocks();
  resetResourceCache();
  useOnboarding().needsOnboarding.value = false;
  daemon.connected.value = true;
  daemon.report.value = makeReport();
  daemon.lastError.value = null;
  mocks.sitesAndParked.mockResolvedValue({ sites: sitesList(62), parked: ["/Users/me/Herd"], groups: NO_GROUPS });
  mocks.daemonVersionConflict.mockResolvedValue(null);
  mocks.releaseNotes.mockResolvedValue({
    type: "release_notes",
    current: release("v2.1.0-rc.1", {
      prerelease: true,
      features: [{ title: "Preferred IDE" }, { title: "Streamed FPM responses" }],
      fixes: [{ title: "certutil found on PATH" }],
    }),
    earlier: ["v2.0.4", "v2.0.3", "v2.0.2", "v2.0.1", "v2.0.0"].map((t) => release(t)),
  });
  mocks.contributors.mockResolvedValue({ type: "contributors", contributors: people(12), total: 12 });
  mocks.stopDaemon.mockResolvedValue(undefined);
  mocks.restartDaemon.mockResolvedValue(undefined);
  mocks.openInBrowser.mockResolvedValue(undefined);
  mocks.refresh.mockResolvedValue(undefined);
  mocks.tunnelStatus.mockResolvedValue({ type: "tunnels", tunnels: [], cloudflared: { installed: false } });
  mocks.startService.mockResolvedValue(undefined);
  mocks.stopService.mockResolvedValue(undefined);
  mocks.restartService.mockResolvedValue(undefined);
  mocks.checkUpdates.mockResolvedValue({ available: false, channel: "stable", source: "live" });
  mocks.setSiteFavourite.mockResolvedValue(undefined);
  sitesIntent.value = null;
  sitesSelectIntent.value = null;
  servicesIntent.value = null;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  document.body.innerHTML = "";
  Reflect.deleteProperty(globalThis, "matchMedia");
});

function rowDomains(w: VueWrapper, scope = ""): string[] {
  return w.findAll(`${scope} [data-testid="site-domain"]`.trim()).map((d) => d.text());
}

async function showRecent(w: VueWrapper): Promise<void> {
  const radio = w.findAll('[role="radio"]').find((r) => r.text() === "Recently added");
  if (!radio) throw new Error("no Recently added option");
  await radio.trigger("click");
  await flushPromises();
}

describe("OverviewView states", () => {
  it("shows the connecting skeleton before the first probe", async () => {
    daemon.connected.value = null;
    daemon.report.value = null;
    const w = await mountView();

    expect(w.find('[data-testid="overview-skeleton"]').exists()).toBe(true);
    expect(w.find('[data-testid="favourites-skeleton"]').exists()).toBe(true);
    expect(w.text()).not.toContain("Yerd isn't running");
  });

  it("keeps the skeleton while connected without a report", async () => {
    daemon.report.value = null;
    const w = await mountView();

    expect(w.find('[data-testid="overview-skeleton"]').exists()).toBe(true);
    expect(w.find('[data-testid="status-card"]').exists()).toBe(false);
  });

  it("shows the status error with Retry instead of a perpetual skeleton", async () => {
    daemon.report.value = null;
    daemon.lastError.value = { message: "unknown variant `status_v2`" };
    const w = await mountView();

    expect(w.find('[data-testid="overview-skeleton"]').exists()).toBe(false);
    const card = w.get('[data-testid="overview-status-error"]');
    expect(card.text()).toContain("unknown variant `status_v2`");
    await buttonByText(w, "Retry").trigger("click");
    await flushPromises();
    expect(mocks.refresh).toHaveBeenCalled();
  });

  it("shows the hero with its footer note while the daemon is down", async () => {
    daemon.connected.value = false;
    const w = await mountView();

    expect(w.text()).toContain("Yerd isn't running");
    expect(w.text()).toContain("Your sites and services come back once Yerd is running.");
    expect(w.text()).toContain("Release notes appear once Yerd is running.");
    expect(w.text()).toContain("Contributors appear once Yerd is running.");
    expect(w.find('[data-testid="services-card"]').exists()).toBe(false);
    expect(buttonByText(w, "New site").attributes("disabled")).toBeDefined();
    expect(buttonByText(w, "Add service").attributes("disabled")).toBeDefined();
    expect(mocks.releaseNotes).not.toHaveBeenCalled();
    expect(mocks.contributors).not.toHaveBeenCalled();
    expect(mocks.sitesAndParked).not.toHaveBeenCalled();
    expect(mocks.tunnelStatus).not.toHaveBeenCalled();
  });

  it("shows the serving environment strip", async () => {
    const w = await mountView();
    const card = w.get('[data-testid="status-card"]');

    expect(card.get('[data-testid="status-label"]').text()).toBe("Serving");
    expect(card.get("h2").text()).toBe("62 sites .test");
    expect(card.text()).toContain("yerdd 2.1.0-rc.1 · up 11m 0s · pid 55921 · 2 of 2 services running");
    const chips = card.findAll('[data-testid="health-rows"] li');
    expect(chips.map((c) => c.text())).toEqual(["Local CA trusted", ".test resolver installed"]);
    expect(chips.map((c) => c.attributes("data-tone"))).toEqual(["ok", "ok"]);
  });

  it("shows NOT SERVING and a Change ports link when the web ports are unbound", async () => {
    daemon.report.value = makeReport({ web_unbound: { http: 8080, https: 8443 } });
    const w = await mountView();
    const card = w.get('[data-testid="status-card"]');

    expect(card.get('[data-testid="status-label"]').text()).toBe("Not serving");
    expect(card.get("h2").text()).toBe("0 of 62 sites .test");
    const bad = card.get('[data-tone="bad"]');
    expect(bad.text()).toContain("Couldn't bind web ports 8080/8443");
    expect(bad.get("a").attributes("href")).toBe("/general");
    expect(bad.get("a").text()).toBe("Change ports");
  });

  it("shows an amber Fix chip when the ports fell back without a redirect", async () => {
    daemon.report.value = makeReport({
      http: { requested: 80, bound: 8080, fell_back: true },
      https: { requested: 443, bound: 8443, fell_back: true },
      port_redirect: false,
      foreign_web_listener: true,
    });
    const w = await mountView();
    const warn = w.get('[data-tone="warn"]');

    expect(warn.text()).toContain("80/443 busy, serving on :8080 / :8443");
    expect(warn.get("a").attributes("href")).toBe("/doctor");
    expect(warn.get("a").text()).toBe("Fix");
  });

  it("shows the setup checklist on an empty environment", async () => {
    daemon.report.value = makeReport({
      php: [],
      sites: { parked: 0, linked: 0, secured: 0 },
    });
    mocks.sitesAndParked.mockResolvedValue({ sites: [], parked: [], groups: NO_GROUPS });
    mocks.pickDirectory.mockResolvedValue("/Users/me/Code");
    mocks.park.mockResolvedValue(undefined);
    const w = await mountView();

    expect(w.get('[data-testid="status-label"]').text()).toBe("Ready");
    expect(w.get('[data-testid="status-card"] h2').text()).toBe("No sites yet");
    const checklist = w.get('[data-testid="setup-checklist"]');
    expect(checklist.get('[data-step="php"] a').attributes("href")).toBe("/php");

    await buttonByText(w, "Choose…").trigger("click");
    await flushPromises();
    expect(mocks.pickDirectory).toHaveBeenCalledOnce();
    expect(mocks.park).toHaveBeenCalledWith("/Users/me/Code");

    await buttonByText(w, "Re-run the full guided setup").trigger("click");
    expect(useOnboarding().needsOnboarding.value).toBe(true);
  });

  it("keeps the checklist after PHP is installed and offers the park step", async () => {
    daemon.report.value = makeReport({ sites: { parked: 0, linked: 0, secured: 0 } });
    mocks.sitesAndParked.mockResolvedValue({ sites: [], parked: [], groups: NO_GROUPS });
    const w = await mountView();

    const checklist = w.get('[data-testid="setup-checklist"]');
    expect(checklist.get('[data-step="php"]').text()).toContain("Done");
    expect(checklist.get('[data-step="park"]').text()).toContain("Choose…");
  });

  it("hides the checklist once PHP is installed and a folder is parked", async () => {
    const w = await mountView();
    expect(w.find('[data-testid="setup-checklist"]').exists()).toBe(false);
  });
});

describe("OverviewView action bar", () => {
  it.each([
    ["New site", "/sites", () => sitesIntent.value, "create"],
    ["Link folder", "/sites", () => sitesIntent.value, "link"],
    ["Add service", "/services", () => servicesIntent.value, "add"],
  ] as const)("%s sets its intent and routes", async (label, path, intent, expected) => {
    const w = await mountView();

    await buttonByText(w, label).trigger("click");
    await flushPromises();
    expect(intent()).toBe(expected);
    expect(router.currentRoute.value.path).toBe(path);
  });

  it("checks for updates and opens GitHub", async () => {
    const w = await mountView();

    await w.get('[aria-label="Check for updates"]').trigger("click");
    await flushPromises();
    expect(mocks.checkUpdates).toHaveBeenCalledOnce();
    await w.get('[aria-label="GitHub"]').trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://github.com/forjedio/yerd");
  });
});

describe("OverviewView pinned sites", () => {
  const groups = { order: ["Acme", "Side"], members: { "acme-api": "Acme", "acme-shop": "Acme", "acme-admin": "Acme", side: "Side" } };

  function grouped(extra: SiteEntry[] = []) {
    return {
      sites: sitesList(62, [
        site("acme-api", { favourite: true }),
        site("acme-shop"),
        site("acme-admin"),
        site("side"),
        site("zeta", { favourite: true, php: "8.4" }),
        site("alpha", { favourite: true, is_wordpress: true }),
        ...extra,
      ]),
      parked: [],
      groups,
    };
  }

  it("groups starred sites under their group and lists the rest below", async () => {
    mocks.sitesAndParked.mockResolvedValue(grouped());
    const w = await mountView();

    const blocks = w.findAll('[data-testid="pinned-group"]');
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.text()).toContain("Acme");
    expect(blocks[0]?.text()).toContain("1/3 sites starred");
    expect(rowDomains(w, '[data-testid="pinned-group"]')).toEqual(["acme-api.test"]);
    expect(rowDomains(w, '[data-testid="pinned-ungrouped"]')).toEqual(["alpha.test", "zeta.test"]);
    expect(w.text()).not.toContain("Side");
    expect(w.text()).toContain("Pinned 3");
    expect(w.text()).toContain("All 62 sites");
  });

  it("shows the path, badges and PHP pill on a row", async () => {
    mocks.sitesAndParked.mockResolvedValue(grouped());
    const w = await mountView();
    const rows = w.findAll('[data-testid="pinned-ungrouped"] [data-testid="favourite-row"]');

    expect(rows[0]?.text()).toContain("~/Herd/alpha");
    expect(rows[0]?.find('[data-testid="wordpress-badge"]').exists()).toBe(true);
    expect(rows[1]?.find('[data-testid="wordpress-badge"]').exists()).toBe(false);
    expect(rows[1]?.get('[data-testid="favourite-php"]').classes()).toContain("text-warning");
    expect(rows[0]?.get('[data-testid="favourite-php"]').classes()).not.toContain("text-warning");
  });

  it("copies the path, opens the site and edits it in Sites", async () => {
    mocks.sitesAndParked.mockResolvedValue(grouped());
    const w = await mountView();
    const row = w.findAll('[data-testid="pinned-ungrouped"] [data-testid="favourite-row"]')[0];

    await row?.get('[data-testid="copy-path"]').trigger("click");
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("/Users/me/Herd/alpha");

    await row?.get('[data-testid="site-open"]').trigger("click");
    await flushPromises();
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://alpha.test");

    await row?.get('[data-testid="site-edit"]').trigger("click");
    await flushPromises();
    expect(sitesSelectIntent.value).toBe("alpha");
    expect(router.currentRoute.value.path).toBe("/sites");
  });

  it("unfavourites from the star without opening the site, then reloads", async () => {
    mocks.sitesAndParked.mockResolvedValue({
      sites: sitesList(3, [site("alpha", { favourite: true })]),
      parked: [],
      groups: NO_GROUPS,
    });
    const w = await mountView();
    const loads = mocks.sitesAndParked.mock.calls.length;

    await w.get('[data-testid="favourite-star"]').trigger("click");
    await flushPromises();

    expect(mocks.setSiteFavourite).toHaveBeenCalledWith("alpha", false);
    expect(mocks.openInBrowser).not.toHaveBeenCalled();
    expect(mocks.sitesAndParked.mock.calls.length).toBeGreaterThan(loads);
  });

  it("shows the empty state when nothing is starred", async () => {
    const w = await mountView();

    expect(w.find('[data-testid="favourites-empty"]').text()).toContain("No favourites yet");
  });

  it("badges a live share, and drops it once nothing is shared", async () => {
    daemon.report.value = makeReport({ shared_sites: 1 });
    mocks.sitesAndParked.mockResolvedValue(grouped());
    mocks.tunnelStatus.mockResolvedValue({
      type: "tunnels",
      tunnels: [{ site: "alpha", kind: "quick", state: "running" }],
      cloudflared: { installed: true },
    });
    const w = await mountView();
    const shared = () => w.findAll('[data-testid="shared-badge"]');

    expect(shared()).toHaveLength(1);
    expect(w.findAll('[data-testid="favourite-row"]')[1]?.find('[data-testid="shared-badge"]').exists()).toBe(true);

    daemon.report.value = makeReport({ shared_sites: 0 });
    await flushPromises();
    expect(shared()).toHaveLength(0);

    let resolve: (v: unknown) => void = () => {};
    mocks.tunnelStatus.mockReturnValue(new Promise((r) => (resolve = r)));
    daemon.report.value = makeReport({ shared_sites: 1 });
    await flushPromises();
    expect(shared()).toHaveLength(0);
    resolve({ type: "tunnels", tunnels: [], cloudflared: { installed: true } });
    await flushPromises();
    expect(shared()).toHaveLength(0);
  });

  it("doesn't fetch tunnels while nothing is shared", async () => {
    await mountView();
    expect(mocks.tunnelStatus).not.toHaveBeenCalled();
  });
});

describe("OverviewView recently added", () => {
  it("lists the seven newest dated sites, newest first, and stars one", async () => {
    const dated = Array.from({ length: 9 }, (_, i) => site(`new${i}`, { added_at: 1000 + i }));
    mocks.sitesAndParked.mockResolvedValue({ sites: sitesList(62, dated), parked: [], groups: NO_GROUPS });
    const w = await mountView();
    await showRecent(w);

    expect(rowDomains(w, '[data-testid="recent-list"]')).toEqual(
      [8, 7, 6, 5, 4, 3, 2].map((i) => `new${i}.test`),
    );
    await w.get('[data-testid="recent-list"] [data-testid="favourite-star"]').trigger("click");
    await flushPromises();
    expect(mocks.setSiteFavourite).toHaveBeenCalledWith("new8", true);
  });

  it("says so when no site has an added time", async () => {
    const w = await mountView();
    await showRecent(w);

    expect(w.get('[data-testid="recent-empty"]').text()).toContain("No recently added sites yet.");
  });
});

describe("OverviewView banner and dialogs", () => {
  it("shows the version-conflict banner and opens the releases page", async () => {
    mocks.daemonVersionConflict.mockResolvedValue("2.2.0");
    const w = await mountView();
    const banner = w.get('[data-testid="version-conflict"]');

    expect(banner.text()).toContain("This Yerd is older than your daemon");
    expect(banner.text()).toContain("2.2.0");
    await buttonByText(w, "Update").trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://github.com/forjedio/yerd/releases");
  });

  it("stops the daemon only after confirming", async () => {
    const w = await mountView();

    await buttonByText(w, "Stop").trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Stop the daemon?");
    expect(document.body.textContent).toContain("This stops all 62 .test sites");

    bodyButton("Cancel").click();
    await flushPromises();
    expect(document.body.textContent).not.toContain("Stop the daemon?");
    expect(mocks.stopDaemon).not.toHaveBeenCalled();

    await buttonByText(w, "Stop").trigger("click");
    await flushPromises();
    bodyButton("Stop daemon").click();
    await flushPromises();
    expect(mocks.stopDaemon).toHaveBeenCalledOnce();
    expect(mocks.refresh).toHaveBeenCalled();
  });

  it("restarts the daemon after confirming", async () => {
    const w = await mountView();

    await buttonByText(w, "Restart").trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Restart the daemon?");
    bodyButton("Restart").click();
    await flushPromises();
    expect(mocks.restartDaemon).toHaveBeenCalledOnce();
  });
});

describe("OverviewView services card", () => {
  beforeEach(() => {
    daemon.report.value = makeReport({
      services: [
        service("mysql", "MySQL", "running", 3306),
        service("s3", "S3 storage", "stopped", 7070),
        service("redis", "Redis", "stopped", 6379, false),
      ],
    });
  });

  it("lists installed services with their state", async () => {
    const w = await mountView();
    const card = w.get('[data-testid="services-card"]');

    expect(card.get('[data-testid="services-summary"]').text()).toBe("1 of 2 running");
    const rows = card.findAll('[data-testid="service-row"]');
    expect(rows.map((r) => r.attributes("data-service"))).toEqual(["mysql", "s3"]);
    expect(rows[0]?.text()).toContain(":3306");
    expect(rows[1]?.text()).toContain("stopped");
    expect(card.get('a[href="/services"]').text()).toContain("Manage");
  });

  it("restarts, stops and starts a service, then refreshes", async () => {
    const w = await mountView();

    await w.get('[aria-label="Restart MySQL"]').trigger("click");
    await flushPromises();
    expect(mocks.restartService).toHaveBeenCalledWith("mysql");
    await w.get('[aria-label="Stop MySQL"]').trigger("click");
    await flushPromises();
    expect(mocks.stopService).toHaveBeenCalledWith("mysql");
    await w.get('[aria-label="Start S3 storage"]').trigger("click");
    await flushPromises();
    expect(mocks.startService).toHaveBeenCalledWith("s3");
    expect(mocks.refresh).toHaveBeenCalledTimes(3);
  });

  it("names each per-site instance by its site's domain", async () => {
    daemon.report.value = makeReport({
      services: [
        { ...service("reverb:blog", "Reverb", "running", 8080), site: "blog" },
        { ...service("reverb:shop", "Reverb", "running", 8081), site: "shop" },
      ],
    });
    const w = await mountView();

    const sites = w.findAll('[data-testid="service-site"]').map((s) => s.text());
    expect(sites).toEqual(["blog.test", "shop.test"]);
    await w.get('[aria-label="Restart Reverb shop.test"]').trigger("click");
    await flushPromises();
    expect(mocks.restartService).toHaveBeenCalledWith("reverb:shop");
  });
});

describe("OverviewView contributors", () => {
  it("shows five tiles and a more tile when compact, and collapses", async () => {
    const w = await mountView();
    const card = w.get('[data-testid="contributors-card"]');

    expect(card.findAll('[data-testid="contributor"]')).toHaveLength(5);
    expect(card.text()).toContain("12 people · by commits");
    expect(card.findAll("button").some((b) => b.text().includes("GitHub"))).toBe(false);
    await card.get('[data-testid="contributors-more"]').trigger("click");
    expect(card.get('[data-testid="contributors-more"]').text()).toBe("+7 more contributors");
    expect(mocks.openInBrowser).toHaveBeenCalledWith(
      "https://github.com/forjedio/yerd/graphs/contributors",
    );

    await card.get('[data-testid="contributors-toggle"]').trigger("click");
    expect(card.get('[data-testid="contributors-toggle"]').attributes("aria-expanded")).toBe("false");
    expect(card.findAll('[data-testid="contributor"]')).toHaveLength(0);
  });

  it("shows three tiles on a wide window", async () => {
    globalThis.matchMedia = vi.fn(() => ({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })) as unknown as typeof globalThis.matchMedia;
    const w = await mountView();

    expect(w.findAll('[data-testid="contributor"]')).toHaveLength(3);
    expect(w.get('[data-testid="contributors-more"]').text()).toBe("+9 more contributors");
  });

  it("swaps a broken avatar for an initial and opens a profile", async () => {
    const w = await mountView();
    const tiles = w.findAll('[data-testid="contributor"]');

    await tiles[0]?.get("img").trigger("error");
    expect(tiles[0]?.get('[data-testid="contributor-initial"]').text()).toBe("U");
    await tiles[2]?.trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://github.com/user2");
  });
});

describe("OverviewView what's new", () => {
  function manyItems(prefix: string, n: number) {
    return Array.from({ length: n }, (_, i) => ({ title: `${prefix} ${i}`, author: `dev${i % 3}` }));
  }

  it("summarises the installed release with only it listed", async () => {
    const w = await mountView();
    const left = w.get('[data-testid="release-current"]');

    expect(left.text()).toContain("v2.1.0-rc.1");
    expect(left.text()).toContain("Installed");
    expect(left.text()).toContain("Pre-release");
    expect(left.text()).toContain("15 Aug 2026");
    expect(left.get('[data-testid="release-summary"]').text()).toBe("Preferred IDE… and 1 more feature");
    expect(w.findAll('[data-testid="release-row"]')).toHaveLength(1);
    expect(w.find('[data-testid="release-new"]').exists()).toBe(false);

    await w.get('[data-testid="release-link"]').trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith(
      "https://github.com/forjedio/yerd/releases/tag/v2.1.0-rc.1",
    );
  });

  it("expands into capped NEW and FIXED lists, stats and every release", async () => {
    mocks.releaseNotes.mockResolvedValue({
      type: "release_notes",
      current: release("v2.1.0-rc.1", {
        features: manyItems("Feature", 9),
        fixes: manyItems("Fix", 15),
        others: manyItems("Other", 2),
      }),
      earlier: ["v2.0.4", "v2.0.3"].map((t) => release(t)),
    });
    const w = await mountView();

    await w.get('[data-testid="release-toggle"]').trigger("click");
    const added = w.get('[data-testid="release-new"]').findAll("li");
    expect(added).toHaveLength(7);
    expect(added[6]?.text()).toBe("+ 3 more features");
    const fixed = w.get('[data-testid="release-fixed"]').findAll("li");
    expect(fixed).toHaveLength(5);
    expect(fixed[4]?.text()).toBe("+ 11 more fixes");
    expect(w.get('[data-testid="release-stats"]').text()).toBe(
      "26 pull requests · 9 features · 15 fixes · 3 contributors",
    );
    expect(w.findAll('[data-testid="release-row"]').map((r) => r.text())).toEqual([
      expect.stringContaining("v2.1.0-rc.1"),
      expect.stringContaining("v2.0.4"),
      expect.stringContaining("v2.0.3"),
    ]);
  });

  it("swaps the left pane to an earlier release", async () => {
    const w = await mountView();

    await w.get('[data-testid="release-toggle"]').trigger("click");
    await w.findAll('[data-testid="release-row"]')[1]?.trigger("click");
    const left = w.get('[data-testid="release-current"]');
    expect(left.text()).toContain("v2.0.4");
    expect(left.text()).not.toContain("Installed");
    expect(left.get('[data-testid="release-summary"]').text()).toBe("Feature in v2.0.4");
  });

  it("says so when no notes are published for the running version", async () => {
    mocks.releaseNotes.mockResolvedValue({ type: "release_notes", earlier: [release("v2.0.4")] });
    const w = await mountView();

    expect(w.get('[data-testid="release-current"]').text()).toContain("v2.1.0-rc.1");
    expect(w.text()).toContain("No release notes published for this version.");
    expect(w.findAll('[data-testid="release-row"]')).toHaveLength(1);
  });

  it("keeps every release listed after picking one when the running version has no notes", async () => {
    mocks.releaseNotes.mockResolvedValue({
      type: "release_notes",
      earlier: [release("v2.0.4"), release("v2.0.3")],
    });
    const w = await mountView();

    await w.findAll('[data-testid="release-row"]')[1]?.trigger("click");
    expect(w.get('[data-testid="release-current"]').text()).toContain("v2.0.3");
    expect(w.findAll('[data-testid="release-row"]')).toHaveLength(2);
  });

  it("says so when release notes fail to load", async () => {
    const { IpcError } = await import("@/ipc/client");
    mocks.releaseNotes.mockRejectedValue(new IpcError("offline"));
    const w = await mountView();

    expect(w.text()).toContain("Couldn't load release notes.");
  });
});
