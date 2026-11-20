import { mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import ShareInspector from "./ShareInspector.vue";
import type { CloudflaredStatus, Site } from "@/ipc/types";
import type { ShareItem } from "@/lib/share";

const mounted: VueWrapper[] = [];

const cloudflared: CloudflaredStatus = { installed: true, version: "2026.6.1", source: "managed", logged_in: true };

function quickItem(overrides: Partial<ShareItem> = {}): ShareItem {
  return {
    key: "quick:app",
    kind: "quick",
    site: "app",
    url: "https://fuzzy.trycloudflare.com",
    host: "fuzzy.trycloudflare.com",
    state: "running",
    protected: false,
    lastError: null,
    warnings: [],
    ...overrides,
  };
}

function domainItem(overrides: Partial<ShareItem> = {}): ShareItem {
  return quickItem({
    key: "domain:app",
    kind: "domain",
    url: "https://app.forjed.io",
    host: "app.forjed.io",
    ...overrides,
  });
}

function site(secure: boolean): Site {
  return { name: "app", document_root: "/srv/app", php: "8.4", secure } as Site;
}

function mountInspector(props: Partial<InstanceType<typeof ShareInspector>["$props"]> = {}) {
  const wrapper = mount(ShareInspector, {
    props: {
      mode: "panel",
      shown: true,
      item: quickItem(),
      site: site(true),
      tld: "test",
      tunnelName: "yerd-mac",
      cloudflared,
      canRestart: false,
      loggedIn: true,
      busy: null,
      connected: true,
      ...props,
    },
    global: { stubs: { teleport: true } },
  });
  mounted.push(wrapper);
  return wrapper;
}

function action(w: VueWrapper, name: string) {
  return w.find(`[data-action="${name}"]`);
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
});

describe("ShareInspector", () => {
  it("shows a running quick link with its URL actions, Stop sharing and the note", async () => {
    const w = mountInspector();
    expect(w.get('[data-testid="share-chip"]').text()).toBe("Quick link");
    expect(w.get('[data-testid="share-subtitle"]').text()).toContain("app.test over HTTPS");
    expect(action(w, "restart").exists()).toBe(false);
    expect(action(w, "remove").exists()).toBe(false);
    expect(w.find('[data-testid="share-note"]').exists()).toBe(true);
    await action(w, "copy").trigger("click");
    await action(w, "open").trigger("click");
    await action(w, "stop").trigger("click");
    expect(w.emitted("copy")).toEqual([["https://fuzzy.trycloudflare.com"]]);
    expect(w.emitted("open")).toEqual([["https://fuzzy.trycloudflare.com"]]);
    expect(w.emitted("stop")).toHaveLength(1);
  });

  it("reads HTTP for an insecure site", () => {
    const w = mountInspector({ site: site(false) });
    expect(w.get('[data-testid="share-subtitle"]').text()).toContain("over HTTP ·");
  });

  it("waits for a quick link's URL with Copy and Open disabled", () => {
    const w = mountInspector({ item: quickItem({ url: null, host: null, state: "connecting" }) });
    expect(w.get('[data-testid="share-url"]').text()).toContain("Waiting for the public URL");
    expect(action(w, "copy").attributes("disabled")).toBeDefined();
    expect(action(w, "open").attributes("disabled")).toBeDefined();
  });

  it("shows a live domain share without Restart or the quick-link note", async () => {
    const w = mountInspector({ item: domainItem() });
    expect(w.get('[data-testid="share-subtitle"]').text()).toContain("Exposed through tunnel yerd-mac");
    expect(action(w, "stop").exists()).toBe(false);
    expect(action(w, "restart").exists()).toBe(false);
    expect(w.find('[data-testid="share-note"]').exists()).toBe(false);
    await action(w, "remove").trigger("click");
    expect(w.emitted("removeFromDomain")).toHaveLength(1);
  });

  it("tints a failed domain share, hides the URL actions and offers Restart", async () => {
    const w = mountInspector({ item: domainItem({ state: "failed", lastError: "dial tcp: refused" }), canRestart: true });
    expect(w.get('[data-testid="share-facts"]').attributes("data-failed")).toBe("true");
    expect(w.get('[data-testid="share-status"]').text()).toContain("dial tcp: refused");
    expect(action(w, "copy").exists()).toBe(false);
    expect(action(w, "restart").text()).toBe("Restart tunnel");
    await action(w, "restart").trigger("click");
    expect(w.emitted("restart")).toHaveLength(1);
  });

  it("offers Start tunnel for an offline domain share, disabled when logged out", () => {
    const w = mountInspector({ item: domainItem({ state: "offline" }), canRestart: true, loggedIn: false });
    expect(action(w, "restart").text()).toBe("Start tunnel");
    expect(action(w, "restart").attributes("disabled")).toBeDefined();
  });

  it("lists the share's warnings", () => {
    const w = mountInspector({ item: quickItem({ warnings: ["Vite dev server is running"] }) });
    expect(w.get('[data-testid="share-warnings"]').text()).toContain("Vite dev server is running");
  });

  it.each([
    [false, "Set a share password"],
    [true, "Password on"],
  ])("titles the password button for protected %s", (isProtected, title) => {
    const w = mountInspector({ item: quickItem({ protected: isProtected }) });
    expect(action(w, "password").attributes("title")).toBe(title);
  });

  it("disables the header actions while the daemon is down", () => {
    const w = mountInspector({ connected: false });
    expect(action(w, "stop").attributes("disabled")).toBeDefined();
    expect(action(w, "password").attributes("disabled")).toBeDefined();
  });

  it("closes the drawer from its close button", async () => {
    const w = mountInspector({ mode: "drawer" });
    await w.get('[aria-label="Close share details"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
  });
});
