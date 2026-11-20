import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  openInBrowser: vi.fn(),
  toggleWindowZoom: vi.fn(),
  hostPlatform: vi.fn(),
  toggleMaximize: vi.fn(),
  isMac: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  openInBrowser: mocks.openInBrowser,
  toggleWindowZoom: mocks.toggleWindowZoom,
  hostPlatform: mocks.hostPlatform,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ toggleMaximize: mocks.toggleMaximize }),
}));

vi.mock("@/lib/shortcuts/platform", () => ({
  isMac: mocks.isMac,
}));

import { loadPlatform } from "@/composables/usePlatform";
import { OPEN_PALETTE_KEY } from "@/lib/shortcuts/registry";

import PageHeader from "./PageHeader.vue";

let wrapper: ReturnType<typeof mount> | null = null;

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
  }
  await nextTick();
}

function mountHeader(opts: { palette?: () => void; docs?: string } = {}) {
  wrapper = mount(PageHeader, {
    props: { title: "Sites", docs: opts.docs ?? "/guide/x" },
    global: opts.palette ? { provide: { [OPEN_PALETTE_KEY as symbol]: opts.palette } } : {},
    slots: { actions: '<button type="button" data-testid="action">Add</button>' },
    attachTo: document.body,
  });
  return wrapper;
}

beforeEach(async () => {
  vi.clearAllMocks();
  mocks.openInBrowser.mockResolvedValue(undefined);
  mocks.toggleWindowZoom.mockResolvedValue(undefined);
  mocks.toggleMaximize.mockResolvedValue(undefined);
  mocks.hostPlatform.mockResolvedValue("macos");
  mocks.isMac.mockReturnValue(true);
  await loadPlatform();
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
});

describe("PageHeader toolbar", () => {
  it("renders the title with no subtitle element", () => {
    const w = mountHeader();

    expect(w.get("h2").text()).toBe("Sites");
    expect(w.find("p").exists()).toBe(false);
  });

  it("hides the search trigger when no palette opener is provided", () => {
    const w = mountHeader();

    expect(w.find('button[aria-label="Search or run a command"]').exists()).toBe(false);
  });

  it("opens the palette from the search trigger with a macOS chord hint", async () => {
    const open = vi.fn();
    const w = mountHeader({ palette: open });

    const trigger = w.get('button[aria-label="Search or run a command"]');
    expect(trigger.get("kbd").text()).toBe("⌘K");
    await trigger.trigger("click");

    expect(open).toHaveBeenCalledOnce();
  });

  it("shows the Ctrl chord hint off macOS", () => {
    mocks.isMac.mockReturnValue(false);
    const w = mountHeader({ palette: vi.fn() });

    expect(w.get("kbd").text()).toBe("Ctrl+K");
  });

  it("opens the docs page in the browser", async () => {
    const w = mountHeader();

    const docs = w.findAll("button").find((b) => b.text().includes("Docs"));
    await docs?.trigger("click");

    expect(mocks.openInBrowser).toHaveBeenCalledWith("https://yerd.app/guide/x");
  });

  it("is a deep drag region with no bare drag attribute below it", () => {
    const w = mountHeader({ palette: vi.fn() });

    expect(w.get("header").attributes("data-tauri-drag-region")).toBe("deep");
    expect(w.findAll("header [data-tauri-drag-region]")).toHaveLength(0);
  });

  it("zooms on a double click on the bar or the title, not on its buttons", async () => {
    const w = mountHeader({ palette: vi.fn() });

    await w.get("header").trigger("dblclick");
    await flush();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledTimes(1);

    await w.get("h2").trigger("dblclick");
    await flush();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledTimes(2);

    const docs = w.findAll("button").find((b) => b.text().includes("Docs"));
    await docs?.trigger("dblclick");
    await w.get('button[aria-label="Search or run a command"]').trigger("dblclick");
    await flush();
    expect(mocks.toggleWindowZoom).toHaveBeenCalledTimes(2);
    expect(mocks.toggleMaximize).not.toHaveBeenCalled();
  });

  it("hides a double click's press from the drag region", () => {
    const w = mountHeader();
    const seen: number[] = [];
    const record = (e: Event) => seen.push((e as MouseEvent).detail);
    document.addEventListener("mousedown", record);

    try {
      const header = w.get("header").element;
      header.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, detail: 1 }));
      header.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, detail: 2 }));
      expect(seen).toEqual([1]);
    } finally {
      document.removeEventListener("mousedown", record);
    }
  });
});
