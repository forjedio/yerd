import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

import IntegrationsView from "./IntegrationsView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { resetResourceCache } from "@/composables/useResource";
import type { SiteEntry, SiteHostname, StatusReport, TunnelInfo } from "@/ipc/types";

function site(name: string): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.3",
    secure: false,
    kind: "linked",
    is_wordpress: false,
  };
}

/** A default mock: the given tunnels/named sites, `ok` for every mutation, and a
 *  reject for anything unexpected so a wrong wire call is loud. */
function stubIpc(opts: {
  tunnels?: TunnelInfo[];
  named?: SiteHostname[];
  sites?: SiteEntry[];
}) {
  const tunnels = opts.tunnels ?? [];
  const named = opts.named ?? [];
  const sites = opts.sites ?? [site("app"), site("blog")];
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "status":
        return Promise.resolve({ type: "status", report: { tld: "test" } as StatusReport });
      case "tunnel_status":
        return Promise.resolve({
          type: "tunnels",
          tunnels,
          cloudflared: { installed: true, version: "2026.6.1", source: "managed", logged_in: true },
        });
      case "list_sites":
        return Promise.resolve({ type: "sites", sites });
      case "list_named_tunnels":
        return Promise.resolve({
          type: "named_tunnels",
          tunnels: [{ name: "dev", uuid: "0000-1111" }],
          sites: named,
          zone: "example.com",
        });
      case "start_named_tunnel":
        return Promise.resolve({
          type: "tunnels",
          tunnels,
          cloudflared: { installed: true, logged_in: true },
        });
      case "set_site_tunnel":
      case "route_tunnel_dns":
      case "set_tunnel_password":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: { unmount: () => void }[] = [];

async function mountView() {
  await useDaemon().refresh();
  const wrapper = mount(IntegrationsView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type Wrapper = Awaited<ReturnType<typeof mountView>>;

function clickByText(wrapper: Wrapper, text: string) {
  const btn = wrapper
    .findAll("button")
    .find((b) => b.text().trim() === text && b.attributes("disabled") === undefined);
  if (!btn) throw new Error(`no enabled button with text "${text}"`);
  return btn.trigger("click");
}

function calls(cmd: string): Record<string, unknown>[] {
  return invokeMock.mock.calls
    .filter((c) => c[0] === cmd)
    .map((c) => c[1] as Record<string, unknown>);
}

function named(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "@named", kind: "named", state, ...extra };
}

function quick(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "app", kind: "quick", state, ...extra };
}

describe("IntegrationsView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
  });

  it("exposes a site by saving the hostname then routing DNS, never starting the tunnel", async () => {
    stubIpc({});
    const wrapper = await mountView();

    const row = wrapper.findAll("tbody tr").find((r) => r.text().includes("blog"));
    if (!row) throw new Error("no blog row");
    await row.findAll("button").find((b) => b.text().includes("Expose"))?.trigger("click");
    await flushPromises();

    expect(calls("set_site_tunnel")).toEqual([{ site: "blog", hostname: "blog.example.com" }]);
    expect(calls("route_tunnel_dns")).toEqual([{ tunnel: "dev", hostname: "blog.example.com" }]);
    const names = invokeMock.mock.calls.map((c) => c[0]);
    expect(names.indexOf("set_site_tunnel")).toBeLessThan(names.indexOf("route_tunnel_dns"));
    expect(names).not.toContain("start_named_tunnel");
    expect(names).not.toContain("stop_named_tunnel");
  });

  it("removing an exposed site only clears its hostname", async () => {
    stubIpc({
      tunnels: [named("running")],
      named: [{ site: "app", hostname: "app.example.com" }],
    });
    const wrapper = await mountView();

    const row = wrapper.findAll("tbody tr").find((r) => r.text().includes("app"));
    if (!row) throw new Error("no app row");
    await row.findAll("button").find((b) => b.text().trim() === "Remove")?.trigger("click");
    await flushPromises();

    expect(calls("set_site_tunnel")).toEqual([{ site: "app", hostname: null }]);
    const names = invokeMock.mock.calls.map((c) => c[0]);
    expect(names).not.toContain("route_tunnel_dns");
    expect(names).not.toContain("start_named_tunnel");
    expect(names).not.toContain("stop_named_tunnel");
  });

  it.each([
    ["connecting", "Connecting"],
    ["running", "Online"],
    ["reconnecting", "Reconnecting"],
    ["failed", "Failed, retrying"],
  ] as const)("renders the named %s state as %s", async (state, label) => {
    stubIpc({
      tunnels: [named(state, { last_error: state === "failed" ? "dial tcp: refused" : undefined })],
      named: [{ site: "app", hostname: "app.example.com" }],
    });
    const wrapper = await mountView();

    expect(wrapper.find('[data-testid="named-state"]').text()).toBe(label);
    if (state === "failed") {
      expect(wrapper.find('[data-testid="named-error"]').text()).toBe("dial tcp: refused");
    }
    const startBtn = wrapper.findAll("button").find((b) => b.text().trim() === "Start");
    expect(startBtn).toBeUndefined();
  });

  it.each([
    ["connecting", "Connecting"],
    ["running", "Online"],
    ["reconnecting", "Reconnecting"],
    ["failed", "Failed, retrying"],
  ] as const)("renders the quick %s state as %s with its last error", async (state, label) => {
    stubIpc({ tunnels: [quick(state, { last_error: "edge unreachable" })] });
    const wrapper = await mountView();

    expect(wrapper.find('[data-testid="tunnel-state"]').text()).toBe(label);
    expect(wrapper.text()).toContain("edge unreachable");
  });

  it("offers Start only when sites are exposed and no named instance exists", async () => {
    stubIpc({ named: [{ site: "app", hostname: "app.example.com" }] });
    const wrapper = await mountView();

    expect(wrapper.text()).toContain("offline");
    await clickByText(wrapper, "Start");
    await flushPromises();

    expect(calls("start_named_tunnel")).toHaveLength(1);
  });

  it("hides Start when nothing is exposed", async () => {
    stubIpc({});
    const wrapper = await mountView();
    const startBtn = wrapper.findAll("button").find((b) => b.text().trim() === "Start");
    expect(startBtn).toBeUndefined();
  });

  it("sets a share password from the modal", async () => {
    stubIpc({ tunnels: [quick("running", { url: "https://x.trycloudflare.com" })] });
    const wrapper = await mountView();

    await wrapper.find('button[aria-label="Share password"]').trigger("click");
    await flushPromises();
    expect(wrapper.text()).not.toContain("Remove password");
    await wrapper.find("#share-password").setValue("s3cret");
    await clickByText(wrapper, "Save");
    await flushPromises();

    expect(calls("set_tunnel_password")).toEqual([{ site: "app", password: "s3cret" }]);
  });

  it("forgets a typed password when the modal is cancelled", async () => {
    stubIpc({ tunnels: [quick("running", { url: "https://x.trycloudflare.com" })] });
    const wrapper = await mountView();

    await wrapper.find('button[aria-label="Share password"]').trigger("click");
    await flushPromises();
    await wrapper.find("#share-password").setValue("s3cret");
    await clickByText(wrapper, "Cancel");
    await flushPromises();

    const vm = wrapper.vm as unknown as { passwordInput: string; passwordSite: string | null };
    expect(vm.passwordInput).toBe("");
    expect(vm.passwordSite).toBeNull();
    expect(calls("set_tunnel_password")).toEqual([]);
  });

  it("removes the share password of a protected named site", async () => {
    stubIpc({
      tunnels: [named("running")],
      named: [{ site: "app", hostname: "app.example.com", protected: true }],
    });
    const wrapper = await mountView();

    expect(wrapper.find('[aria-label="Password protected"]').exists()).toBe(true);
    await wrapper.find('button[aria-label="Share password"]').trigger("click");
    await flushPromises();
    await clickByText(wrapper, "Remove password");
    await flushPromises();

    expect(calls("set_tunnel_password")).toEqual([{ site: "app", password: null }]);
  });

  it("marks a protected quick share", async () => {
    stubIpc({ tunnels: [quick("running", { protected: true })] });
    const wrapper = await mountView();
    expect(wrapper.find('[aria-label="Password protected"]').exists()).toBe(true);
  });

  it("renders quick and per-site named warnings", async () => {
    stubIpc({
      tunnels: [
        quick("running", { warnings: ["Vite dev server is running"] }),
        named("running", { warnings: ["blog: SESSION_DOMAIN is pinned to .test"] }),
      ],
      named: [{ site: "blog", hostname: "blog.example.com" }],
    });
    const wrapper = await mountView();

    expect(wrapper.text()).toContain("Vite dev server is running");
    const blogRow = wrapper.findAll("tbody tr").find((r) => r.text().includes("blog"));
    expect(blogRow?.text()).toContain("SESSION_DOMAIN is pinned to .test");
    expect(blogRow?.text()).not.toContain("blog: SESSION_DOMAIN");
  });
});
