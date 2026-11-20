import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { StatusReport, UpdateStatusResponse } from "@/ipc/types";

const mocks = vi.hoisted(() => ({
  cachedUpdateStatus: vi.fn(),
  checkUpdates: vi.fn(),
  setUpdateChannel: vi.fn(),
  applyUpdate: vi.fn(),
  protocolVersion: vi.fn(),
  status: vi.fn(),
  openInBrowser: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  ...mocks,
}));

vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(async () => "2.1.0") }));

vi.mock("@/composables/useDaemon", async () => {
  const { ref } = await import("vue");
  const state = { connected: ref<boolean | null>(true) };
  return { useDaemon: () => state, daemonState: state };
});

import * as daemonModule from "@/composables/useDaemon";

import AboutSection from "./AboutSection.vue";

const daemon = (daemonModule as unknown as { daemonState: { connected: { value: boolean | null } } })
  .daemonState;

const mounted: VueWrapper[] = [];
const LAYOUTS = ["column", "card"] as const;

function upd(over: Partial<UpdateStatusResponse> = {}): UpdateStatusResponse {
  return {
    type: "update_status",
    current: "2.1.0-rc.1",
    latest_stable: "2.0.4",
    latest_edge: "2.1.0",
    channel: "edge",
    available: false,
    target: null,
    ahead_of_stable: false,
    source: "live",
    checked_at_epoch: Date.now() / 1000 - 240,
    ...over,
  };
}

async function mountAbout(layout: "column" | "card") {
  const w = mount(AboutSection, { props: { layout } });
  mounted.push(w);
  await flushPromises();
  return w;
}

function button(w: VueWrapper, label: string) {
  return w.findAll("button").find((b) => b.text().trim() === label);
}

beforeEach(() => {
  daemon.connected.value = true;
  mocks.cachedUpdateStatus.mockReset().mockResolvedValue(upd());
  mocks.checkUpdates.mockReset().mockResolvedValue(upd());
  mocks.setUpdateChannel.mockReset().mockResolvedValue(undefined);
  mocks.applyUpdate.mockReset();
  mocks.protocolVersion.mockReset().mockResolvedValue(1);
  mocks.status
    .mockReset()
    .mockResolvedValue({ daemon_version: "2.1.0-rc.1" } as unknown as StatusReport);
  mocks.openInBrowser.mockReset();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
});

describe("AboutSection", () => {
  it.each(LAYOUTS)("%s: renders the versions", async (layout) => {
    const w = await mountAbout(layout);
    expect(w.find('[data-testid="about-app-version"]').text()).toBe("2.1.0");
    expect(w.find('[data-testid="about-daemon-version"]').text()).toBe("2.1.0-rc.1");
    expect(w.find('[data-testid="about-protocol"]').text()).toBe("1");
  });

  it.each(LAYOUTS)("%s: up to date offers Check now, not Update", async (layout) => {
    const w = await mountAbout(layout);
    expect(w.find('[data-testid="update-headline"]').text()).toContain("Up to date");
    expect(button(w, "Check now")).toBeDefined();
    expect(button(w, "Update")).toBeUndefined();
    await button(w, "Check now")?.trigger("click");
    await flushPromises();
    expect(mocks.checkUpdates).toHaveBeenCalledTimes(1);
  });

  it.each(LAYOUTS)("%s: an available update offers Update instead", async (layout) => {
    let fail: (e: Error) => void = () => {};
    mocks.cachedUpdateStatus.mockResolvedValue(upd({ available: true, target: "2.1.0" }));
    mocks.applyUpdate.mockReturnValue(new Promise((_, reject) => (fail = reject)));
    const w = await mountAbout(layout);
    expect(w.find('[data-testid="update-headline"]').text()).toContain("Yerd 2.1.0 is available");
    expect(button(w, "Check now")).toBeUndefined();
    await button(w, "Update")?.trigger("click");
    expect(mocks.applyUpdate).toHaveBeenCalledWith("edge");
    expect(button(w, "Updating…")?.attributes("disabled")).toBeDefined();
    fail(new Error("offline"));
    await flushPromises();
  });

  it("keeps Update disabled when it remounts mid-apply", async () => {
    let fail: (e: Error) => void = () => {};
    mocks.cachedUpdateStatus.mockResolvedValue(upd({ available: true, target: "2.1.0" }));
    mocks.applyUpdate.mockReturnValue(new Promise((_, reject) => (fail = reject)));
    const first = await mountAbout("column");
    await button(first, "Update")?.trigger("click");
    first.unmount();
    mounted.length = 0;

    const second = await mountAbout("card");
    expect(button(second, "Updating…")?.attributes("disabled")).toBeDefined();

    fail(new Error("offline"));
    await flushPromises();
    expect(button(second, "Update")?.attributes("disabled")).toBeUndefined();
  });

  it.each(LAYOUTS)("%s: daemon down disables the channel and Check now", async (layout) => {
    daemon.connected.value = false;
    mocks.cachedUpdateStatus.mockRejectedValue(new Error("down"));
    const w = await mountAbout(layout);
    expect(w.find('[data-testid="update-headline"]').text()).toContain("Start the daemon");
    expect(button(w, "Check now")?.attributes("disabled")).toBeDefined();
    expect(w.find('[role="radiogroup"]').attributes("data-disabled")).toBeDefined();
  });

  it.each(LAYOUTS)("%s: switching channel saves then re-checks", async (layout) => {
    const w = await mountAbout(layout);
    await w.findAll('[role="radio"]')[0]?.trigger("click");
    await flushPromises();
    expect(mocks.setUpdateChannel).toHaveBeenCalledWith("stable");
    expect(mocks.checkUpdates).toHaveBeenCalledTimes(1);
  });

  it.each(LAYOUTS)("%s: each link opens its URL", async (layout) => {
    const w = await mountAbout(layout);
    for (const label of ["yerd.app", "GitHub", "MIT licence", "mdbin.app"]) {
      await button(w, label)?.trigger("click");
    }
    expect(mocks.openInBrowser.mock.calls.map((c) => c[0])).toEqual([
      "https://yerd.app",
      "https://github.com/forjedio/yerd",
      "https://github.com/forjedio/yerd/blob/main/LICENSE.md",
      "https://mdbin.app/?utm=yerdapp",
    ]);
  });

  it.each(LAYOUTS)("%s: Logs and Diagnostics emit their events", async (layout) => {
    const w = await mountAbout(layout);
    await button(w, "Logs")?.trigger("click");
    await button(w, "Diagnostics")?.trigger("click");
    expect(w.emitted("logs")).toHaveLength(1);
    expect(w.emitted("diagnostics")).toHaveLength(1);
  });
});
