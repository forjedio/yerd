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
  daemonSelfRepairBusy: vi.fn(),
}));

// Captures the callback each `listen(name, cb)` call registers, keyed by event
// name, so tests can fire `daemon-start-phase` / `daemon-self-repair` events
// independently (the previous single shared mock couldn't distinguish them).
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());

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
    daemonSelfRepairBusy: mocks.daemonSelfRepairBusy,
    status: (...args: unknown[]) => mocks.statusImpl(...args),
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, cb: (event: { payload: unknown }) => void) => {
    listeners.set(name, cb);
    return () => {};
  }),
}));
vi.mock("@/lib/log", () => ({
  log: { info() {}, debug() {}, warn() {}, error() {} },
}));

/** Flush the microtask queue - fake timers only stub `setTimeout`/`setInterval`,
 * not native promise resolution, so this settles `ensureListener`'s awaited
 * `listen()`/seed chain without advancing any timers. */
async function flushMicrotasks(times = 10): Promise<void> {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
  }
}

import { IpcError } from "@/ipc/client";

// POLL_MS / RUNNING_CEILING_MS are module-private in useDaemonStart.ts; mirror
// them here so the ceiling assertions stay pinned to the same granularity.
const POLL_MS = 500;
const RUNNING_CEILING_MS = 30_000;

let activeWrapper: ReturnType<typeof mount> | null = null;

/**
 * Mount a fresh instance of the composable. `phase`/`backgroundBusy`/
 * `listenerStarted` are module-level singletons in `useDaemonStart.ts`, so a
 * static top-level import would only ever register `listen()` once for the
 * whole file - later tests could never observe their own `daemon-self-repair`
 * callback. `vi.resetModules()` (in `beforeEach` below) plus a dynamic import
 * here gives every test its own fresh singleton, matching one real app launch.
 */
async function mountComposable() {
  const { useDaemonStart } = await import("./useDaemonStart");
  let api!: ReturnType<typeof useDaemonStart>;
  const Comp = defineComponent({
    setup() {
      api = useDaemonStart();
      return () => h("div");
    },
  });
  activeWrapper = mount(Comp);
  await flushMicrotasks();
  return { wrapper: activeWrapper, api };
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
  vi.resetModules();
  vi.clearAllMocks();
  listeners.clear();
  mocks.startDaemon.mockResolvedValue(undefined);
  mocks.daemonDiagnostics.mockResolvedValue(diag);
  mocks.daemonSelfRepairBusy.mockResolvedValue(false);
  vi.useFakeTimers();
});

afterEach(() => {
  activeWrapper?.unmount();
  activeWrapper = null;
  vi.useRealTimers();
});

describe("useDaemonStart readiness wait", () => {
  it("connects before the ceiling and shows no diagnostics panel", async () => {
    mocks.statusImpl.mockResolvedValue({});
    const { api } = await mountComposable();

    await api.start();

    expect(mocks.daemonDiagnostics).not.toHaveBeenCalled();
    expect(api.diagnostics.value).toBeNull();
    expect(api.phase.value).toBe("idle");
  });

  it("only surfaces diagnostics after the 30s ceiling, not before", async () => {
    mocks.statusImpl.mockRejectedValue(new IpcError("down", "unreachable"));
    const { api } = await mountComposable();

    void api.start();
    // One poll short of the ceiling: still waiting, no diagnostics yet.
    await vi.advanceTimersByTimeAsync(RUNNING_CEILING_MS - POLL_MS);
    expect(mocks.daemonDiagnostics).not.toHaveBeenCalled();
    expect(api.phase.value).toBe("running");

    // The poll that crosses the ceiling surfaces diagnostics exactly once.
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(mocks.daemonDiagnostics).toHaveBeenCalledOnce();
    expect(api.diagnostics.value).not.toBeNull();
    expect(api.phase.value).toBe("idle");
  });
});

describe("useDaemonStart background self-repair signal", () => {
  it("daemon-self-repair true shows busy/Preparing Daemon; false clears it", async () => {
    mocks.statusImpl.mockResolvedValue({});
    const { api } = await mountComposable();

    listeners.get("daemon-self-repair")?.({ payload: true });
    expect(api.starting.value).toBe(true);
    expect(api.activeLabel.value).toBe("Preparing Daemon");

    listeners.get("daemon-self-repair")?.({ payload: false });
    expect(api.starting.value).toBe(false);
    expect(api.activeLabel.value).toBeNull();
  });

  it("a late-resolving seed can't clobber a since-arrived event (seed race guard)", async () => {
    let resolveSeed!: (busy: boolean) => void;
    mocks.daemonSelfRepairBusy.mockImplementation(
      () =>
        new Promise<boolean>((resolve) => {
          resolveSeed = resolve;
        }),
    );
    const { api } = await mountComposable();

    // The thread finishes and emits its terminal `false` before the seed IPC
    // (still in flight) resolves.
    listeners.get("daemon-self-repair")?.({ payload: false });
    expect(api.starting.value).toBe(false);

    // The seed now resolves late with a stale `true` sampled before the thread
    // finished. Without the `backgroundBusySeen` guard this would incorrectly
    // flip `starting` back to true with no further event to correct it.
    resolveSeed(true);
    await flushMicrotasks();

    expect(api.starting.value).toBe(false);
    expect(api.activeLabel.value).toBeNull();
  });

  it("start() is gated on phase alone - a same-tick self-repair doesn't block it", async () => {
    mocks.statusImpl.mockResolvedValue({});
    const { api } = await mountComposable();

    listeners.get("daemon-self-repair")?.({ payload: true });
    expect(api.starting.value).toBe(true);

    await api.start();
    expect(mocks.startDaemon).toHaveBeenCalledOnce();
  });
});
