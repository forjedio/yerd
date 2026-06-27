import { mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the IPC client (used by both useDaemonStart and the real useDaemon store,
// whose `status` probe drives `connected`). Keeping the real useDaemon gives us
// genuine reactivity for the `watch(connected)` auto-clear.
const mocks = vi.hoisted(() => ({
  statusImpl: vi.fn(),
  startDaemon: vi.fn(),
  daemonDiagnostics: vi.fn(),
}));

vi.mock("@/ipc/client", () => {
  class IpcError extends Error {
    unreachable: boolean;
    constructor(message: string, code = "internal") {
      super(message);
      this.message = message;
      this.unreachable = code === "unreachable";
    }
  }
  return {
    IpcError,
    startDaemon: mocks.startDaemon,
    daemonDiagnostics: mocks.daemonDiagnostics,
    status: (...args: unknown[]) => mocks.statusImpl(...args),
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/log", () => ({
  log: { info() {}, debug() {}, warn() {}, error() {} },
}));

import { IpcError } from "@/ipc/client";
import { useDaemonStart } from "./useDaemonStart";

function mountComposable() {
  let api!: ReturnType<typeof useDaemonStart>;
  const Comp = defineComponent({
    setup() {
      api = useDaemonStart();
      return () => h("div");
    },
  });
  const wrapper = mount(Comp);
  return { wrapper, api };
}

const diag = {
  startError: null,
  hints: [],
  yerddPath: null,
  translocated: false,
  socketPath: "",
  socketResponding: false,
  lastConnectError: null,
  serviceManager: "launchd",
  serviceStatus: null,
  pendingApproval: false,
  logPath: null,
  logTail: [],
  spawnLogTail: [],
  repairLogTail: [],
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.startDaemon.mockResolvedValue(undefined);
  mocks.daemonDiagnostics.mockResolvedValue(diag);
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useDaemonStart readiness wait", () => {
  it("connects before the ceiling and shows no diagnostics panel", async () => {
    mocks.statusImpl.mockResolvedValue({}); // status OK => connected
    const { api } = mountComposable();

    await api.start();

    expect(mocks.daemonDiagnostics).not.toHaveBeenCalled();
    expect(api.diagnostics.value).toBeNull();
    expect(api.phase.value).toBe("idle");
  });

  it("only surfaces diagnostics after the 30s ceiling, not before", async () => {
    mocks.statusImpl.mockRejectedValue(new IpcError("down", "unreachable"));
    const { api } = mountComposable();

    void api.start();
    await vi.advanceTimersByTimeAsync(29_500);
    expect(mocks.daemonDiagnostics).not.toHaveBeenCalled();
    expect(api.phase.value).toBe("running");

    await vi.advanceTimersByTimeAsync(1_500);
    expect(mocks.daemonDiagnostics).toHaveBeenCalledOnce();
    expect(api.diagnostics.value).not.toBeNull();
    expect(api.phase.value).toBe("idle");
  });
});
