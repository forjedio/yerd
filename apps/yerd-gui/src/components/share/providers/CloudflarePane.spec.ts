import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import CloudflarePane from "./CloudflarePane.vue";
import type { CloudflaredStatus, TunnelInfo } from "@/ipc/types";

const mounted: VueWrapper[] = [];

const managed: CloudflaredStatus = { installed: true, version: "2026.6.1", source: "managed", logged_in: true };
const tunnel = { name: "yerd-mac", uuid: "6f1c-2a9e" };

function named(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "@named", kind: "named", state, ...extra };
}

function mountDialog(props: Partial<InstanceType<typeof CloudflarePane>["$props"]> = {}) {
  const wrapper = mount(CloudflarePane, {
    attachTo: document.body,
    props: {
      cloudflared: managed,
      zone: "forjed.io",
      tunnel,
      entry: named("running"),
      exposedCount: 2,
      busy: null,
      connected: true,
      ...props,
    },
  });
  mounted.push(wrapper);
  return wrapper;
}

function section(name: string): HTMLElement {
  return document.querySelector(`[data-testid="setup-${name}"]`) as HTMLElement;
}

function buttonIn(name: string, text: string): HTMLButtonElement | undefined {
  return Array.from(section(name).querySelectorAll("button")).find((b) => b.textContent?.trim() === text);
}

function act(name: string): HTMLButtonElement | null {
  return document.querySelector(`[data-action="${name}"]`);
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("CloudflarePane", () => {
  it("points to Tooling when cloudflared is missing", async () => {
    const w = mountDialog({ cloudflared: { installed: false, logged_in: false }, tunnel: null, entry: null, exposedCount: 0 });
    await flushPromises();
    buttonIn("cloudflared", "Open Tooling")?.click();
    expect(w.emitted("tooling")).toHaveLength(1);
  });

  it.each([
    [managed, false, "Yerd's copy · used for every tunnel"],
    [{ ...managed, update_available: "2026.7.0" }, true, "Yerd's copy · update to 2026.7.0 available in Tooling"],
    [{ ...managed, source: "system" as const, update_available: "2026.7.0" }, false, "Found on PATH · used for every tunnel"],
  ])("links to Tooling only when there is something to do there", async (status, linked, subline) => {
    const w = mountDialog({ cloudflared: status });
    await flushPromises();
    const buttons = section("cloudflared").querySelectorAll("button");
    expect(Array.from(buttons).map((b) => b.textContent?.trim())).toEqual(linked ? ["Open Tooling"] : []);
    expect(document.querySelector('[data-testid="cloudflared-source"]')?.textContent?.trim()).toBe(subline);
    if (linked) {
      buttons[0]?.click();
      expect(w.emitted("tooling")).toHaveLength(1);
    }
  });

  it("connects a logged-out account and locks Remove tunnel", async () => {
    const w = mountDialog({ cloudflared: { ...managed, logged_in: false } });
    await flushPromises();
    buttonIn("account", "Connect")?.click();
    expect(w.emitted("login")).toHaveLength(1);
    expect(act("remove")?.disabled).toBe(true);
  });

  it("asks for a login before a tunnel can be created", async () => {
    mountDialog({ cloudflared: { ...managed, logged_in: false }, tunnel: null, entry: null, exposedCount: 0 });
    await flushPromises();
    expect(buttonIn("tunnel", "Create")).toBeUndefined();
    expect(section("tunnel").textContent).toContain("Connect your Cloudflare account first.");
  });

  it("creates a tunnel with the trimmed name", async () => {
    const w = mountDialog({ tunnel: null, entry: null, exposedCount: 0 });
    await flushPromises();
    expect(buttonIn("tunnel", "Create")?.disabled).toBe(true);
    const input = section("tunnel").querySelector("input") as HTMLInputElement;
    input.value = "  yerd-mac ";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    buttonIn("tunnel", "Create")?.click();
    expect(w.emitted("create")).toEqual([["yerd-mac"]]);
  });

  it("shows a live tunnel without Start or Restart", async () => {
    mountDialog();
    await flushPromises();
    expect(document.querySelector('[data-testid="tunnel-chip"]')?.textContent?.trim()).toBe("Live · 2 sites");
    expect(act("start")).toBeNull();
  });

  it("restarts a failed tunnel and shows its error", async () => {
    const w = mountDialog({ entry: named("failed", { last_error: "dial tcp: refused" }) });
    await flushPromises();
    expect(document.querySelector('[data-testid="tunnel-error"]')?.textContent?.trim()).toBe("dial tcp: refused");
    expect(act("start")?.textContent?.trim()).toBe("Restart");
    act("start")?.click();
    expect(w.emitted("start")).toHaveLength(1);
  });

  it("starts a stopped tunnel with exposed sites, but not while logged out", async () => {
    mountDialog({ entry: null });
    await flushPromises();
    expect(act("start")?.textContent?.trim()).toBe("Start");
    expect(act("start")?.disabled).toBe(false);
    mounted.forEach((w) => w.unmount());
    document.body.innerHTML = "";
    mountDialog({ entry: null, cloudflared: { ...managed, logged_in: false } });
    await flushPromises();
    expect(act("start")?.disabled).toBe(true);
  });

  it.each([
    [managed, "Ready"],
    [{ installed: false, logged_in: false }, "Not installed"],
  ])("shows the readiness chip %#", async (status, label) => {
    mountDialog({ cloudflared: status });
    await flushPromises();
    expect(document.querySelector('[data-testid="pane-readiness"]')?.textContent?.trim()).toBe(label);
  });

  it("opens the Cloudflare dashboard through the view", async () => {
    const w = mountDialog();
    await flushPromises();
    Array.from(document.querySelectorAll("button"))
      .find((b) => b.textContent?.includes("dash.cloudflare.com"))
      ?.click();
    expect(w.emitted("open-url")).toEqual([["https://dash.cloudflare.com"]]);
  });

  it("asks to remove the tunnel", async () => {
    const w = mountDialog();
    await flushPromises();
    act("remove")?.click();
    expect(w.emitted("remove")).toHaveLength(1);
  });
});
