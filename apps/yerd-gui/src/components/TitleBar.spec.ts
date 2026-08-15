import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The window controls are a thin wrapper over Tauri's window API, so the whole
// surface is faked: `isMaximized` is the IPC round trip the resize debounce
// exists to collapse, and `onResized` hands the test the WM callback to fire.
const mocks = vi.hoisted(() => ({
  isMaximized: vi.fn(),
  toggleMaximize: vi.fn(),
  isFocused: vi.fn(),
  onFocusChanged: vi.fn(),
  onResized: vi.fn(),
  hostPlatform: vi.fn(),
  setGuiMaximized: vi.fn(),
  resized: null as null | (() => void),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "main",
    close: vi.fn(),
    minimize: vi.fn(),
    isMaximized: mocks.isMaximized,
    toggleMaximize: mocks.toggleMaximize,
    isFocused: mocks.isFocused,
    onFocusChanged: mocks.onFocusChanged,
    onResized: mocks.onResized,
  }),
}));

vi.mock("@/ipc/client", () => ({
  hostPlatform: mocks.hostPlatform,
  setGuiMaximized: mocks.setGuiMaximized,
  getTitleBarStyle: vi.fn(async () => "auto"),
  setTitleBarStyle: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

import TitleBar from "./TitleBar.vue";

/** The debounce window in `TitleBar.vue`, mirrored here so the boundary
 *  assertions stay pinned to the same granularity. */
const DEBOUNCE_MS = 150;

/** Flush the microtask queue - fake timers stub `setTimeout` only, so this
 *  settles the mounted `hostPlatform()`/`onResized()` promises (and Vue's
 *  scheduler) without advancing any timers. */
async function flushMicrotasks(times = 10): Promise<void> {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
  }
  await nextTick();
}

let wrapper: ReturnType<typeof mount> | null = null;

/**
 * Mount the titlebar on a host that reports a reliable maximized state
 * (`linux`), then clear the mounted-time calls so each test counts only its
 * own `isMaximized()` round trips.
 */
async function mountTitleBar() {
  wrapper = mount(TitleBar);
  await flushMicrotasks();
  mocks.isMaximized.mockClear();
  return wrapper;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.resized = null;
  mocks.isMaximized.mockResolvedValue(false);
  mocks.toggleMaximize.mockResolvedValue(undefined);
  mocks.isFocused.mockResolvedValue(true);
  mocks.onFocusChanged.mockResolvedValue(() => {});
  mocks.onResized.mockImplementation(async (cb: () => void) => {
    mocks.resized = cb;
    return () => {};
  });
  mocks.hostPlatform.mockResolvedValue("linux");
  mocks.setGuiMaximized.mockResolvedValue(undefined);
  vi.useFakeTimers();
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  vi.useRealTimers();
});

describe("TitleBar resize debounce", () => {
  it("collapses an edge-drag's event storm into one isMaximized() read", async () => {
    await mountTitleBar();

    for (let i = 0; i < 25; i++) {
      mocks.resized?.();
    }
    expect(mocks.isMaximized).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(DEBOUNCE_MS - 1);
    expect(mocks.isMaximized).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.isMaximized).toHaveBeenCalledOnce();
  });

  it("reads again for a second gesture once the window has elapsed", async () => {
    await mountTitleBar();

    mocks.resized?.();
    await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
    expect(mocks.isMaximized).toHaveBeenCalledOnce();

    mocks.resized?.();
    await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
    expect(mocks.isMaximized).toHaveBeenCalledTimes(2);
  });

  it("refreshes immediately on toggleMaximize, without waiting for the timer", async () => {
    const w = await mountTitleBar();

    await w.get('button[aria-label="Maximize"]').trigger("click");
    await flushMicrotasks();

    expect(mocks.toggleMaximize).toHaveBeenCalledOnce();
    expect(mocks.isMaximized).toHaveBeenCalledOnce();
  });

  it("clears the pending timer on unmount", async () => {
    const w = await mountTitleBar();

    mocks.resized?.();
    w.unmount();
    wrapper = null;

    await vi.advanceTimersByTimeAsync(DEBOUNCE_MS * 4);
    expect(mocks.isMaximized).not.toHaveBeenCalled();
  });
});
