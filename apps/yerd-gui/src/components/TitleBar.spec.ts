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
  toggleWindowZoom: vi.fn(),
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
  toggleWindowZoom: mocks.toggleWindowZoom,
  getTitleBarStyle: vi.fn(async () => "auto"),
  setTitleBarStyle: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

import { useTitleBarStyle } from "@/lib/titleBarStyle";

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

/** A click's press/release pair. `detail` is read-only on a `UIEvent`, so the
 *  click count has to come from the constructor rather than `trigger()`. */
function pressAndRelease(el: Element, detail: number): void {
  for (const type of ["mousedown", "mouseup"]) {
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, detail }));
  }
}

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

/**
 * Mount the titlebar for a given host, attached to the document so events
 * bubble as far as the document-level listener Tauri's drag-region script
 * installs in a real webview.
 */
async function mountOn(platform: string) {
  mocks.hostPlatform.mockResolvedValue(platform);
  wrapper = mount(TitleBar, { attachTo: document.body });
  await flushMicrotasks();
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
  mocks.toggleWindowZoom.mockResolvedValue(undefined);
  vi.useFakeTimers();
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  useTitleBarStyle().style.value = "auto";
  vi.useRealTimers();
});

/** Aria labels of the window-control buttons, in DOM order. */
function controlLabels(w: ReturnType<typeof mount>): string[] {
  return w
    .findAll("button")
    .map((b) => b.attributes("aria-label") ?? "")
    .filter((l) => ["Close", "Minimize", "Maximize", "Restore", "Zoom"].includes(l));
}

/** True when `a` comes before `b` in document order. */
function precedes(a: Element, b: Element): boolean {
  return (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
}

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

describe("TitleBar zoom gesture", () => {
  it("zooms through the host command on macOS", async () => {
    const w = await mountOn("macos");

    await w.get('button[aria-label="Zoom"]').trigger("click");
    await flushMicrotasks();

    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();
    expect(mocks.toggleMaximize).not.toHaveBeenCalled();
  });

  it("keeps Tauri's own maximize off macOS", async () => {
    const w = await mountOn("linux");

    await w.get('button[aria-label="Maximize"]').trigger("click");
    await flushMicrotasks();

    expect(mocks.toggleMaximize).toHaveBeenCalledOnce();
    expect(mocks.toggleWindowZoom).not.toHaveBeenCalled();
  });

  it("zooms on a double click on the bar, but not through a control", async () => {
    const w = await mountOn("macos");

    await w.get("header").trigger("dblclick");
    await flushMicrotasks();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();

    await w.get('button[aria-label="Zoom"]').trigger("dblclick");
    await flushMicrotasks();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();
  });

  it("zooms on a double click in the gaps around the controls", async () => {
    const w = await mountOn("macos");

    const controls = w.get('button[aria-label="Close"]').element.parentElement;
    expect(controls).not.toBeNull();
    controls?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await flushMicrotasks();

    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();
  });

  it("hides a double click's press and release from the drag region", async () => {
    const w = await mountOn("macos");
    const seen: string[] = [];
    const record = (e: Event) => seen.push(`${e.type}:${(e as MouseEvent).detail}`);
    document.addEventListener("mousedown", record);
    document.addEventListener("mouseup", record);

    try {
      pressAndRelease(w.get("header").element, 1);
      expect(seen).toEqual(["mousedown:1", "mouseup:1"]);

      pressAndRelease(w.get("header").element, 2);
      expect(seen).toEqual(["mousedown:1", "mouseup:1"]);
    } finally {
      document.removeEventListener("mousedown", record);
      document.removeEventListener("mouseup", record);
    }
  });
});

describe("TitleBar left-hand controls", () => {
  it("renders Linux controls close, minimize, maximize before the title", async () => {
    const w = await mountOn("linux");

    expect(controlLabels(w)).toEqual(["Close", "Minimize", "Maximize"]);
    const title = w.get("span.pointer-events-none").element;
    for (const button of w.findAll("button")) {
      expect(precedes(button.element, title)).toBe(true);
    }
  });

  it("renders Windows controls minimize, maximize, close on the left with no logo", async () => {
    useTitleBarStyle().style.value = "windows";
    const w = await mountOn("linux");

    expect(controlLabels(w)).toEqual(["Minimize", "Maximize", "Close"]);
    const title = w.get("span.pointer-events-none").element;
    for (const button of w.findAll("button")) {
      expect(precedes(button.element, title)).toBe(true);
    }
    expect(w.find("img").exists()).toBe(false);
  });

  it("keeps every control ahead of the actions slot", async () => {
    mocks.hostPlatform.mockResolvedValue("linux");
    wrapper = mount(TitleBar, {
      slots: { actions: '<button type="button" data-testid="action">Clear</button>' },
    });
    await flushMicrotasks();

    const action = wrapper.get('[data-testid="action"]').element;
    const controls = wrapper.findAll("button").filter((b) => b.element !== action);
    expect(controls).toHaveLength(3);
    for (const button of controls) {
      expect(precedes(button.element, action)).toBe(true);
    }
  });

  it("skips the state refresh when the maximize toggle fails", async () => {
    const w = await mountTitleBar();
    mocks.toggleMaximize.mockRejectedValue(new Error("no window"));

    await w.get('button[aria-label="Maximize"]').trigger("click");
    await flushMicrotasks();

    expect(mocks.toggleMaximize).toHaveBeenCalledOnce();
    expect(mocks.isMaximized).not.toHaveBeenCalled();
  });
});

describe("TitleBar sidebar variant", () => {
  async function mountSidebar(platform: string) {
    mocks.hostPlatform.mockResolvedValue(platform);
    wrapper = mount(TitleBar, {
      props: { variant: "sidebar", title: "Hidden title" },
      slots: { default: '<div data-testid="brand">Brand</div>' },
      attachTo: document.body,
    });
    await flushMicrotasks();
    return wrapper;
  }

  it("renders the controls and slot content in a deep drag region, without a title", async () => {
    const w = await mountSidebar("macos");

    expect(controlLabels(w)).toEqual(["Close", "Minimize", "Zoom"]);
    expect(w.find('[data-testid="brand"]').exists()).toBe(true);
    expect(w.text()).not.toContain("Hidden title");
    expect(w.find("header").exists()).toBe(false);
    expect(w.attributes("data-tauri-drag-region")).toBe("deep");
  });

  it("zooms on a double click on the slot content, but not on a control", async () => {
    const w = await mountSidebar("macos");

    await w.get('[data-testid="brand"]').trigger("dblclick");
    await flushMicrotasks();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();

    await w.get('button[aria-label="Close"]').trigger("dblclick");
    await flushMicrotasks();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledOnce();
  });

  it("hides a double click's press on the slot content from the drag region", async () => {
    const w = await mountSidebar("macos");
    const seen: number[] = [];
    const record = (e: Event) => seen.push((e as MouseEvent).detail);
    document.addEventListener("mousedown", record);

    try {
      const brand = w.get('[data-testid="brand"]').element;
      brand.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, detail: 2 }));
      expect(seen).toEqual([]);
    } finally {
      document.removeEventListener("mousedown", record);
    }
  });
});
