import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter, type Router } from "vue-router";

import type { StatusReport } from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  cachedUpdateStatus: vi.fn(),
  listPhp: vi.fn(),
  showMailsWindow: vi.fn(),
  insightsStatus: vi.fn(),
  hostPlatform: vi.fn(),
}));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = {
    connected: ref<boolean | null>(null),
    report: ref<StatusReport | null>(null),
  };
  return { useDaemon: () => state, daemonState: state };
});

vi.mock("@/ipc/client", () => ({
  cachedUpdateStatus: mocks.cachedUpdateStatus,
  listPhp: mocks.listPhp,
  showMailsWindow: mocks.showMailsWindow,
  insightsStatus: mocks.insightsStatus,
  hostPlatform: mocks.hostPlatform,
  getTitleBarStyle: vi.fn(async () => "auto"),
  setTitleBarStyle: vi.fn(async () => {}),
  setGuiMaximized: vi.fn(async () => {}),
  toggleWindowZoom: vi.fn(async () => {}),
  IpcError: class IpcError extends Error {},
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "main",
    close: vi.fn(),
    minimize: vi.fn(),
    isMaximized: vi.fn(async () => false),
    toggleMaximize: vi.fn(async () => {}),
    isFocused: vi.fn(async () => true),
    onFocusChanged: vi.fn(async () => () => {}),
    onResized: vi.fn(async () => () => {}),
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

import * as daemonModule from "@/composables/useDaemon";
import { resetResourceCache } from "@/composables/useResource";

import SideNav from "./SideNav.vue";

const daemon = (
  daemonModule as unknown as {
    daemonState: {
      connected: { value: boolean | null };
      report: { value: StatusReport | null };
    };
  }
).daemonState;

const PATHS = [
  "/overview",
  "/sites",
  "/proxies",
  "/php",
  "/node",
  "/tooling",
  "/services",
  "/mail",
  "/insights",
  "/integrations",
  "/doctor",
  "/general",
];

function makeReport(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    sites: { parked: 5, linked: 2, secured: 0 },
    daemon_version: "2.1.0",
    mail: { unread: 0 },
    shared_sites: 0,
    node_updates: 0,
    ca: { trusted_system: true },
    resolver_installed: true,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ...overrides,
  } as unknown as StatusReport;
}

let wrapper: ReturnType<typeof mount> | null = null;
let router: Router;

async function mountNav() {
  router = createRouter({
    history: createMemoryHistory(),
    routes: PATHS.map((path) => ({ path, component: { template: "<div />" } })),
  });
  await router.push("/overview");
  wrapper = mount(SideNav, { global: { plugins: [router] } });
  await flushPromises();
  return wrapper;
}

function labels(w: ReturnType<typeof mount>, list: string): string[] {
  return w
    .get(`[data-testid="${list}"]`)
    .findAll("a")
    .map((a) => a.get("span.truncate").text());
}

function row(w: ReturnType<typeof mount>, label: string) {
  const found = w.findAll("a").find((a) => a.text().startsWith(label));
  if (!found) throw new Error(`no nav row ${label}`);
  return found;
}

beforeEach(() => {
  vi.clearAllMocks();
  resetResourceCache();
  daemon.connected.value = true;
  daemon.report.value = makeReport();
  mocks.cachedUpdateStatus.mockResolvedValue({ available: false });
  mocks.listPhp.mockResolvedValue({ updates: [] });
  mocks.insightsStatus.mockResolvedValue({ open_issues: 0 });
  mocks.hostPlatform.mockResolvedValue("macos");
  mocks.showMailsWindow.mockResolvedValue(undefined);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
});

describe("SideNav", () => {
  it("lists the views in draft order with Doctor and Settings pinned", async () => {
    const w = await mountNav();

    expect(labels(w, "nav-main")).toEqual([
      "Overview",
      "Sites",
      "Proxies",
      "PHP",
      "Node",
      "Tooling",
      "Services",
      "Mail",
      "Insights",
      "Share",
    ]);
    expect(labels(w, "nav-pinned")).toEqual(["Doctor", "Settings"]);
  });

  it("has no section headings", async () => {
    const w = await mountNav();

    for (const heading of ["General", "Environment", "Developer", "Integrations", "System"]) {
      expect(w.text()).not.toContain(heading);
    }
  });

  it("counts parked plus linked sites on the Sites row", async () => {
    const w = await mountNav();

    expect(row(w, "Sites").get('[data-testid="nav-badge"]').text()).toBe("7");
  });

  it("opens the mail viewer from the Mail pill without navigating", async () => {
    daemon.report.value = makeReport({ mail: { unread: 4 } } as Partial<StatusReport>);
    const w = await mountNav();

    await row(w, "Mail").get('[data-testid="nav-badge"]').trigger("click");
    await flushPromises();

    expect(mocks.showMailsWindow).toHaveBeenCalledOnce();
    expect(router.currentRoute.value.path).toBe("/overview");
  });

  it("badges Insights with the open-issue count from insights_status", async () => {
    mocks.insightsStatus.mockResolvedValue({ open_issues: 3 });
    const w = await mountNav();

    expect(row(w, "Insights").get('[data-testid="nav-badge"]').text()).toBe("3");
    expect(row(w, "Insights").find('[data-testid="nav-state"]').exists()).toBe(false);
  });

  it("hides the Insights badge at zero open issues", async () => {
    const w = await mountNav();

    expect(row(w, "Insights").find('[data-testid="nav-badge"]').exists()).toBe(false);
  });

  it("omits the Insights badge while there is no status report", async () => {
    mocks.insightsStatus.mockResolvedValue({ open_issues: 5 });
    daemon.report.value = null;
    const w = await mountNav();

    expect(row(w, "Insights").find('[data-testid="nav-badge"]').exists()).toBe(false);
  });

  it("links Insights to /insights", async () => {
    const w = await mountNav();

    expect(row(w, "Insights").attributes("href")).toBe("/insights");
  });

  it("marks Doctor when a privilege needs elevating", async () => {
    daemon.report.value = makeReport({ resolver_installed: false });
    const w = await mountNav();

    expect(row(w, "Doctor").find('[data-testid="nav-warn"]').exists()).toBe(true);
  });

  it("leaves Doctor unmarked when everything is elevated", async () => {
    const w = await mountNav();

    expect(row(w, "Doctor").find('[data-testid="nav-warn"]').exists()).toBe(false);
  });

  it("shows PHP and Node update counts", async () => {
    mocks.listPhp.mockResolvedValue({ updates: [{}, {}] });
    daemon.report.value = makeReport({ node_updates: 3 });
    const w = await mountNav();

    expect(row(w, "PHP").get('[data-testid="nav-badge"]').text()).toBe("2");
    expect(row(w, "Node").get('[data-testid="nav-badge"]').text()).toBe("3");
  });

  it("badges Settings when a Yerd update is available", async () => {
    mocks.cachedUpdateStatus.mockResolvedValue({ available: true });
    const w = await mountNav();

    expect(row(w, "Settings").get('[data-testid="nav-badge"]').text()).toBe("1");
  });

  it.each([
    [true, "Running"],
    [false, "Unreachable"],
    [null, "Connecting…"],
  ] as const)("reads the daemon status line when connected is %s", async (connected, text) => {
    daemon.connected.value = connected;
    const w = await mountNav();

    expect(w.get('[data-testid="daemon-status"]').text()).toBe(text);
    expect(w.find('[data-testid="daemon-version"]').exists()).toBe(connected === true);
  });

  it("renders the window controls inside the sidebar", async () => {
    const w = await mountNav();

    expect(w.get("nav").find('button[aria-label="Close"]').exists()).toBe(true);
    expect(w.get("nav").find('[data-tauri-drag-region="deep"]').exists()).toBe(true);
  });
});
