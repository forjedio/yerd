import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter, type Router } from "vue-router";

import type {
  Contributor,
  ReleaseNotesEntry,
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
  refresh: vi.fn(),
}));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = {
    connected: ref<boolean | null>(null),
    report: ref<StatusReport | null>(null),
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

import OverviewView from "./OverviewView.vue";

const daemon = (
  daemonModule as unknown as {
    daemonState: {
      connected: { value: boolean | null };
      report: { value: StatusReport | null };
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
    services: [{ state: "running" }, { state: "running" }],
    ...overrides,
  } as unknown as StatusReport;
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

let wrapper: VueWrapper | null = null;
let router: Router;

async function mountView(): Promise<VueWrapper> {
  router = createRouter({
    history: createMemoryHistory(),
    routes: ["/overview", "/sites", "/doctor", "/general", "/php"].map((path) => ({
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
  mocks.sitesAndParked.mockResolvedValue({ sites: sitesList(62), parked: ["/Users/me/Herd"], groups: {} });
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
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  document.body.innerHTML = "";
});

describe("OverviewView left column", () => {
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

  it("shows the hero with its footer note while the daemon is down", async () => {
    daemon.connected.value = false;
    const w = await mountView();

    expect(w.text()).toContain("Yerd isn't running");
    expect(w.text()).toContain("Favourites and services come back once Yerd is running.");
    expect(w.text()).toContain("Release notes appear once Yerd is running.");
    expect(w.text()).toContain("Contributors appear once Yerd is running.");
    expect(mocks.releaseNotes).not.toHaveBeenCalled();
    expect(mocks.contributors).not.toHaveBeenCalled();
    expect(mocks.sitesAndParked).not.toHaveBeenCalled();
  });

  it("shows the serving status card", async () => {
    const w = await mountView();
    const card = w.get('[data-testid="status-card"]');

    expect(card.get('[data-testid="status-label"]').text()).toBe("Serving");
    expect(card.get("h2").text()).toBe("62 sites .test");
    expect(card.text()).toContain("yerdd 2.1.0-rc.1 · up 11m 0s · pid 55921 · 2 services running");
    const rows = card.findAll('[data-testid="health-rows"] li');
    expect(rows).toHaveLength(1);
    expect(rows[0]?.text()).toBe("Local CA trusted · .test resolver installed");
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

  it("shows an amber Fix row when the ports fell back without a redirect", async () => {
    daemon.report.value = makeReport({
      http: { requested: 80, bound: 8080, fell_back: true },
      https: { requested: 443, bound: 8443, fell_back: true },
      port_redirect: false,
      foreign_web_listener: true,
    });
    const w = await mountView();
    const warn = w.get('[data-tone="warn"]');

    expect(warn.text()).toContain("Ports 80/443 are in use — serving on :8080 / :8443");
    expect(warn.get("a").attributes("href")).toBe("/doctor");
  });

  it("shows the setup checklist on an empty environment", async () => {
    daemon.report.value = makeReport({
      php: [],
      sites: { parked: 0, linked: 0, secured: 0 },
    });
    mocks.sitesAndParked.mockResolvedValue({ sites: [], parked: [], groups: {} });
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

  it("lists only starred sites, sorted, and opens one on click", async () => {
    mocks.sitesAndParked.mockResolvedValue({
      sites: sitesList(62, [
        site("zeta", { favourite: true, php: "8.4" }),
        site("alpha", { favourite: true, kind: "linked", framework: "wordpress" }),
      ]),
      parked: [],
      groups: {},
    });
    const w = await mountView();
    const rows = w.findAll('[data-testid="favourite-row"]');

    expect(rows.map((r) => r.get("span.font-mono").text())).toEqual(["alpha.test", "zeta.test"]);
    expect(rows[0]?.text()).toContain("/Users/me/Herd/alpha · linked · WordPress");
    expect(rows[1]?.get('[data-testid="favourite-php"]').classes()).toContain("text-warning");
    expect(rows[0]?.get('[data-testid="favourite-php"]').classes()).not.toContain("text-warning");
    expect(w.text()).toContain("2 starred");
    expect(w.text()).toContain("All 62 sites");

    await rows[0]?.trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://alpha.test");
  });

  it("shows the empty state when nothing is starred", async () => {
    const w = await mountView();

    expect(w.find('[data-testid="favourites-empty"]').text()).toContain("No favourites yet");
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

describe("OverviewView right column", () => {
  it("renders the current release and two earlier releases", async () => {
    const w = await mountView();
    const current = w.get('[data-testid="release-current"]');

    expect(current.text()).toContain("v2.1.0-rc.1");
    expect(current.text()).toContain("Installed");
    expect(current.text()).toContain("Pre-release");
    expect(current.text()).toContain("15 Aug 2026");
    expect(current.text()).toContain("Preferred IDE");
    expect(current.text()).toContain("certutil found on PATH");

    const rows = w.findAll('[data-testid="release-earlier-row"]');
    expect(rows).toHaveLength(2);
    expect(w.text()).not.toContain("Feature in v2.0.4");
    await rows[0]?.trigger("click");
    expect(rows[0]?.attributes("aria-expanded")).toBe("true");
    expect(w.text()).toContain("Feature in v2.0.4");
  });

  it("says so when release notes fail to load", async () => {
    const { IpcError } = await import("@/ipc/client");
    mocks.releaseNotes.mockRejectedValue(new IpcError("offline"));
    const w = await mountView();

    expect(w.text()).toContain("Couldn't load release notes.");
  });

  it("shows nine of the contributors and swaps a broken avatar for an initial", async () => {
    const w = await mountView();
    const tiles = w.findAll('[data-testid="contributor"]');

    expect(tiles).toHaveLength(9);
    expect(w.get('[data-testid="contributors-card"]').text()).toContain("12 people · by commits");
    await tiles[0]?.get("img").trigger("error");
    expect(tiles[0]?.get('[data-testid="contributor-initial"]').text()).toBe("U");

    await buttonByText(w, "More contributors").trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith(
      "https://github.com/forjedio/yerd/graphs/contributors",
    );
  });

  it("opens a contributor's GitHub profile when their tile is clicked", async () => {
    const w = await mountView();

    await w.findAll('[data-testid="contributor"]')[2]?.trigger("click");
    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://github.com/user2");
  });
});
