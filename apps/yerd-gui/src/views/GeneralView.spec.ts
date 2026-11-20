import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AutostartState, StatusReport } from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  refresh: vi.fn(),
  applyAndRestart: vi.fn(),
  validate: vi.fn(),
  validateLoopback: vi.fn(),
  cliPathStatus: vi.fn(),
  daemonInfo: vi.fn(),
  dumpsStatus: vi.fn(),
  getAutostart: vi.fn(),
  getPreferredIde: vi.fn(),
  getTrayIconVariant: vi.fn(),
  getInstalledIdes: vi.fn(),
  installCliToPath: vi.fn(),
  removeCliFromPath: vi.fn(),
  openLoginItems: vi.fn(),
  setAutostartDaemon: vi.fn(),
  setAutostartGui: vi.fn(),
  setAutostartGuiMinimized: vi.fn(),
  setMcpEnabled: vi.fn(),
  setPreferredIde: vi.fn(),
  setSymlinkProtection: vi.fn(),
  setTrayIconVariant: vi.fn(),
  hostPlatform: vi.fn(),
}));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = {
    connected: ref<boolean | null>(true),
    report: ref<StatusReport | null>(null),
    refresh: mocks.refresh,
  };
  return { useDaemon: () => state, daemonState: state };
});

vi.mock("@/composables/useFallbackPorts", async (importActual) => {
  const actual = await importActual<typeof import("@/composables/useFallbackPorts")>();
  return {
    ...actual,
    useFallbackPorts: () => ({
      applyAndRestart: mocks.applyAndRestart,
      validate: mocks.validate,
      validateLoopback: mocks.validateLoopback,
    }),
  };
});

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  homeDirectory: vi.fn(async () => "/Users/me"),
  openInBrowser: vi.fn(),
  getTitleBarStyle: vi.fn(async () => "auto"),
  setTitleBarStyle: vi.fn(async () => {}),
  cliPathStatus: mocks.cliPathStatus,
  daemonInfo: mocks.daemonInfo,
  dumpsStatus: mocks.dumpsStatus,
  getAutostart: mocks.getAutostart,
  getPreferredIde: mocks.getPreferredIde,
  getTrayIconVariant: mocks.getTrayIconVariant,
  getInstalledIdes: mocks.getInstalledIdes,
  installCliToPath: mocks.installCliToPath,
  removeCliFromPath: mocks.removeCliFromPath,
  openLoginItems: mocks.openLoginItems,
  setAutostartDaemon: mocks.setAutostartDaemon,
  setAutostartGui: mocks.setAutostartGui,
  setAutostartGuiMinimized: mocks.setAutostartGuiMinimized,
  setMcpEnabled: mocks.setMcpEnabled,
  setPreferredIde: mocks.setPreferredIde,
  setSymlinkProtection: mocks.setSymlinkProtection,
  setTrayIconVariant: mocks.setTrayIconVariant,
  hostPlatform: mocks.hostPlatform,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "main", setTheme: vi.fn(async () => {}) }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

import * as daemonModule from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { useTheme } from "@/lib/theme";

import GeneralView from "./GeneralView.vue";

const daemon = (
  daemonModule as unknown as {
    daemonState: {
      connected: { value: boolean | null };
      report: { value: StatusReport | null };
    };
  }
).daemonState;

const Pass = { template: "<div><slot /></div>" };
const dialogStub = (name: string) => ({
  name,
  props: ["open"],
  template: `<div data-testid="${name}" :data-open="String(open)" />`,
});
const AboutStub = {
  name: "AboutSection",
  props: ["layout", "updates"],
  emits: ["logs", "diagnostics"],
  template: `<div :data-testid="'about-' + layout">
    <button @click="$emit('logs')">Logs</button>
    <button @click="$emit('diagnostics')">Diagnostics</button>
  </div>`,
};

const mounted: VueWrapper[] = [];

function makeReport(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    daemon_version: "2.1.0",
    dns_addr: "127.0.0.1:1053",
    mail: { port: 2525 },
    port_redirect: false,
    symlink_protection: true,
    mcp_enabled: false,
    web_unbound: null,
    dns_unbound: null,
    ...overrides,
  } as unknown as StatusReport;
}

function makeAutostart(overrides: Partial<AutostartState> = {}): AutostartState {
  return {
    daemon: true,
    daemonSupported: true,
    gui: true,
    guiMinimized: false,
    daemonPendingApproval: false,
    guiPendingApproval: false,
    ...overrides,
  };
}

function setWide(wide: boolean): void {
  globalThis.matchMedia = vi.fn(() => ({
    matches: wide,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof globalThis.matchMedia;
}

async function mountView() {
  const w = mount(GeneralView, {
    attachTo: document.body,
    global: {
      stubs: {
        DialogPortal: Pass,
        RouterLink: { props: ["to"], template: '<a :href="to"><slot /></a>' },
        NetworkProxyCard: true,
        AboutSection: AboutStub,
        ConnectAgentDialog: dialogStub("connect-agent"),
        GuiLogsDialog: dialogStub("gui-logs"),
        DiagnosticsDialog: dialogStub("diagnostics"),
      },
    },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

function button(w: VueWrapper, label: string) {
  const found = w.findAll("button").find((b) => b.text().trim() === label);
  if (!found) throw new Error(`no button ${label}`);
  return found;
}

function bodyButton(label: string): HTMLButtonElement {
  const found = Array.from(document.body.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no button ${label}`);
  return found;
}

function switchFor(w: VueWrapper, label: string) {
  return w.get(`[role="switch"][aria-label="${label}"]`);
}

beforeEach(() => {
  setWide(false);
  daemon.connected.value = true;
  daemon.report.value = makeReport();
  for (const m of Object.values(mocks)) m.mockReset();
  mocks.refresh.mockResolvedValue(undefined);
  mocks.applyAndRestart.mockResolvedValue({ ok: true });
  mocks.validate.mockReturnValue(null);
  mocks.validateLoopback.mockImplementation((_label: string, n: number) =>
    n >= 1 && n <= 65535 ? null : "Port out of range",
  );
  mocks.cliPathStatus.mockResolvedValue({ installed: true });
  mocks.daemonInfo.mockResolvedValue({ fallback_http: 8080, fallback_https: 8443, dns_port: 1053 });
  mocks.dumpsStatus.mockResolvedValue({ port: 2304 });
  mocks.getAutostart.mockResolvedValue(makeAutostart());
  mocks.getPreferredIde.mockResolvedValue(null);
  mocks.getTrayIconVariant.mockResolvedValue("auto");
  mocks.getInstalledIdes.mockResolvedValue([]);
  mocks.hostPlatform.mockResolvedValue("macos");
  for (const fn of [
    mocks.installCliToPath,
    mocks.removeCliFromPath,
    mocks.openLoginItems,
    mocks.setAutostartDaemon,
    mocks.setAutostartGui,
    mocks.setAutostartGuiMinimized,
    mocks.setMcpEnabled,
    mocks.setPreferredIde,
    mocks.setSymlinkProtection,
    mocks.setTrayIconVariant,
  ]) {
    fn.mockResolvedValue(undefined);
  }
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("GeneralView layout", () => {
  it("shows the About card in the column when compact", async () => {
    const w = await mountView();
    expect(w.find('[data-testid="about-card"]').exists()).toBe(true);
    expect(w.find("aside").exists()).toBe(false);
  });

  it("moves About into the right column when wide", async () => {
    setWide(true);
    const w = await mountView();
    expect(w.find('aside [data-testid="about-column"]').exists()).toBe(true);
    expect(w.find('[data-testid="about-card"]').exists()).toBe(false);
  });

  it("opens the Logs and Diagnostics dialogs from About", async () => {
    const w = await mountView();
    await button(w, "Logs").trigger("click");
    await button(w, "Diagnostics").trigger("click");
    expect(w.get('[data-testid="gui-logs"]').attributes("data-open")).toBe("true");
    expect(w.get('[data-testid="diagnostics"]').attributes("data-open")).toBe("true");
  });
});

describe("GeneralView banners", () => {
  it("opens Login Items from the daemon approval banner", async () => {
    mocks.getAutostart.mockResolvedValue(makeAutostart({ daemonPendingApproval: true }));
    const w = await mountView();
    expect(w.find('[data-testid="banner-daemon-approval"]').exists()).toBe(true);
    await button(w, "Open Login Items").trigger("click");
    expect(mocks.openLoginItems).toHaveBeenCalledTimes(1);
  });

  it("shows the web-unbound banner only while running", async () => {
    daemon.report.value = makeReport({ web_unbound: { http: 8080, https: 8443 } });
    const w = await mountView();
    expect(w.find('[data-testid="banner-web-unbound"]').exists()).toBe(true);
    daemon.connected.value = false;
    await flushPromises();
    expect(w.find('[data-testid="banner-web-unbound"]').exists()).toBe(false);
  });
});

describe("GeneralView startup and appearance", () => {
  it("wires the startup switches", async () => {
    const w = await mountView();
    await switchFor(w, "Run the Yerd daemon in the background").trigger("click");
    await switchFor(w, "Start the Yerd app at login").trigger("click");
    await switchFor(w, "Start the Yerd app minimized").trigger("click");
    await flushPromises();
    expect(mocks.setAutostartDaemon).toHaveBeenCalledWith(false);
    expect(mocks.setAutostartGui).toHaveBeenCalledWith(false);
    expect(mocks.setAutostartGuiMinimized).toHaveBeenCalledWith(true);
  });

  it("disables Start minimized without app-at-login", async () => {
    mocks.getAutostart.mockResolvedValue(makeAutostart({ gui: false }));
    const w = await mountView();
    expect(switchFor(w, "Start the Yerd app minimized").attributes("disabled")).toBeDefined();
  });

  it("sets the theme from the segmented control", async () => {
    const { pref, setTheme } = useTheme();
    const before = pref.value;
    const w = await mountView();
    const light = w.findAll('[role="radiogroup"][aria-label="Theme"] [role="radio"]')[1];
    await light?.trigger("click");
    expect(pref.value).toBe("light");
    setTheme(before);
  });
});

describe("GeneralView application ports", () => {
  it("enables Save only after an edit", async () => {
    const w = await mountView();
    expect(button(w, "Save & restart").attributes("disabled")).toBeDefined();
    await w.get("#ap-dns").setValue("1054");
    expect(button(w, "Save & restart").attributes("disabled")).toBeUndefined();
  });

  it("toasts an invalid edit without opening the confirm", async () => {
    const w = await mountView();
    await w.get("#ap-dns").setValue("70000");
    await button(w, "Save & restart").trigger("click");
    await flushPromises();
    expect(useToast().toasts.value.map((t) => t.title)).toContain("Invalid ports");
    expect(document.body.textContent).not.toContain("Change application ports?");
  });

  it("confirms and applies only the changed port", async () => {
    const w = await mountView();
    await w.get("#ap-dns").setValue("1054");
    await button(w, "Save & restart").trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Change application ports?");
    const confirm = Array.from(document.body.querySelectorAll('[role="dialog"] button')).find(
      (b) => b.textContent?.trim() === "Save & restart",
    ) as HTMLButtonElement;
    confirm.click();
    await flushPromises();
    expect(mocks.applyAndRestart).toHaveBeenCalledWith({ dns: 1054 });
  });

  it("locks HTTP and HTTPS while elevated and links to Doctor", async () => {
    daemon.report.value = makeReport({ port_redirect: true });
    const w = await mountView();
    expect(w.get("#ap-http").attributes("disabled")).toBeDefined();
    expect(w.get("#ap-https").attributes("disabled")).toBeDefined();
    expect(w.get("#ap-dns").attributes("disabled")).toBeUndefined();
    expect(w.get('[data-testid="ports-elevated"] a').attributes("href")).toBe("/doctor");
  });
});

describe("GeneralView integrations and security", () => {
  it("offers Connect an agent only while MCP is on", async () => {
    const off = await mountView();
    expect(off.findAll("button").some((b) => b.text().includes("Connect an agent"))).toBe(false);
    off.unmount();
    mounted.length = 0;

    daemon.report.value = makeReport({ mcp_enabled: true });
    const on = await mountView();
    await button(on, "Connect an agent").trigger("click");
    expect(on.get('[data-testid="connect-agent"]').attributes("data-open")).toBe("true");
  });

  it("removes the CLI from PATH when installed", async () => {
    const w = await mountView();
    await button(w, "Remove").trigger("click");
    await flushPromises();
    expect(mocks.removeCliFromPath).toHaveBeenCalledTimes(1);
  });

  it("confirms before turning symlink protection off", async () => {
    const w = await mountView();
    await switchFor(w, "Symlink protection").trigger("click");
    await flushPromises();
    expect(mocks.setSymlinkProtection).not.toHaveBeenCalled();
    bodyButton("Cancel").click();
    await flushPromises();
    expect(mocks.setSymlinkProtection).not.toHaveBeenCalled();

    await switchFor(w, "Symlink protection").trigger("click");
    await flushPromises();
    bodyButton("Turn off").click();
    await flushPromises();
    expect(mocks.setSymlinkProtection).toHaveBeenCalledWith(false);
  });

  it("turns symlink protection on directly", async () => {
    daemon.report.value = makeReport({ symlink_protection: false });
    const w = await mountView();
    await switchFor(w, "Symlink protection").trigger("click");
    await flushPromises();
    expect(mocks.setSymlinkProtection).toHaveBeenCalledWith(true);
  });
});
