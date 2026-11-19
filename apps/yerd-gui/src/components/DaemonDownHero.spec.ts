import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DaemonDiagnostics, StatusReport } from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  openLoginItems: vi.fn(),
  start: vi.fn(),
  writeText: vi.fn(),
}));

vi.mock("@/composables/useDaemonStart", async () => {
  const { ref } = await import("vue");
  const state = {
    starting: ref(false),
    activeLabel: ref<string | null>(null),
    pendingApproval: ref(false),
    diagnostics: ref<DaemonDiagnostics | null>(null),
    start: mocks.start,
  };
  return { useDaemonStart: () => state, startState: state };
});

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = { report: ref<StatusReport | null>(null) };
  return { useDaemon: () => state };
});

vi.mock("@/ipc/client", () => ({
  openLoginItems: mocks.openLoginItems,
  IpcError: class IpcError extends Error {},
}));

import * as startModule from "@/composables/useDaemonStart";

import DaemonDownHero from "./DaemonDownHero.vue";

const start = (
  startModule as unknown as {
    startState: {
      starting: { value: boolean };
      activeLabel: { value: string | null };
      pendingApproval: { value: boolean };
      diagnostics: { value: DaemonDiagnostics | null };
    };
  }
).startState;

function diagnostics(overrides: Partial<DaemonDiagnostics> = {}): DaemonDiagnostics {
  return {
    startError: null,
    hints: ["Port 1053 is already in use"],
    yerddPath: "/Applications/Yerd.app/yerdd",
    translocated: false,
    socketPath: "/tmp/yerd.sock",
    socketResponding: false,
    lastConnectError: null,
    serviceManager: "launchd",
    serviceStatus: "state = not running",
    pendingApproval: false,
    logPath: "~/Library/Logs/Yerd/yerdd.log",
    logTail: ["INFO booting", "WARN retrying", "ERROR bind 127.0.0.1:1053: address in use"],
    spawnLogTail: [],
    repairLogTail: [],
    ...overrides,
  };
}

let wrapper: VueWrapper | null = null;

function mountHero(slot?: string): VueWrapper {
  wrapper = mount(DaemonDownHero, { slots: slot ? { default: slot } : {} });
  return wrapper;
}

function button(w: VueWrapper, text: string) {
  const found = w.findAll("button").find((b) => b.text().trim() === text);
  if (!found) throw new Error(`no button "${text}"`);
  return found;
}

beforeEach(() => {
  vi.clearAllMocks();
  start.starting.value = false;
  start.activeLabel.value = null;
  start.pendingApproval.value = false;
  start.diagnostics.value = null;
  mocks.openLoginItems.mockResolvedValue(undefined);
  mocks.writeText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText: mocks.writeText },
    configurable: true,
  });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
});

describe("DaemonDownHero", () => {
  it("starts the daemon with a Login Items nudge", async () => {
    const w = mountHero();

    expect(w.text()).toContain("Yerd isn't running");
    await button(w, "Start Yerd").trigger("click");
    expect(mocks.start).toHaveBeenCalledWith({ nudge: true });
  });

  it("shows the active phase and disables the button while starting", () => {
    start.starting.value = true;
    start.activeLabel.value = "Installing Daemon";
    const w = mountHero();

    expect(button(w, "Installing Daemon").attributes("disabled")).toBeDefined();
  });

  it("puts the Login Items panel before an enabled waiting button", async () => {
    start.pendingApproval.value = true;
    const w = mountHero();
    const panel = w.get('[data-testid="login-items-panel"]');

    expect(panel.text()).toContain("One more step");
    const waiting = button(w, "Waiting for approval…");
    expect(waiting.attributes("disabled")).toBeUndefined();
    expect(
      panel.element.compareDocumentPosition(waiting.element) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    await button(w, "Open Login Items").trigger("click");
    expect(mocks.openLoginItems).toHaveBeenCalledOnce();
  });

  it("shows hints, the log excerpt and the raw details on failure", async () => {
    start.diagnostics.value = diagnostics();
    const w = mountHero();

    expect(w.text()).toContain("The daemon didn't come up");
    expect(w.text()).toContain("Port 1053 is already in use");
    const excerpt = w.get('[data-testid="diagnostics-excerpt"]');
    expect(excerpt.text()).not.toContain("INFO booting");
    expect(excerpt.text()).toContain("WARN retrying");
    expect(excerpt.text()).toContain("ERROR bind 127.0.0.1:1053: address in use");
    expect(excerpt.text()).toContain("~/Library/Logs/Yerd/yerdd.log");
    expect(w.get("details").text()).toContain("state = not running");

    await button(w, "Copy diagnostics").trigger("click");
    await flushPromises();
    expect(mocks.writeText).toHaveBeenCalledOnce();
    expect(mocks.writeText.mock.calls[0]?.[0]).toContain("Yerd daemon diagnostics");
  });

  it("omits the details without raw data", () => {
    start.diagnostics.value = diagnostics({ serviceStatus: null, logTail: [] });
    const w = mountHero();

    expect(w.find("details").exists()).toBe(false);
    expect(w.find('[data-testid="diagnostics-excerpt"]').exists()).toBe(false);
  });

  it("renders the default slot as a footer note", () => {
    const w = mountHero("Favourites come back later.");

    expect(w.text()).toContain("Favourites come back later.");
  });
});
