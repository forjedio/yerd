import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const openUrlMock = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => openUrlMock(...args),
}));

import ProxiesView from "./ProxiesView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { resetResourceCache } from "@/composables/useResource";
import type { ProxyEntry, ProxyRuleEntry, SiteEntry, StatusReport } from "@/ipc/types";

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

/** A minimal bound, resolver-installed daemon report - enough for the URL
 *  helpers, so a proxy's "Open" affordance is enabled (not unbound). */
function boundReport(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    dns_unbound: null,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    port_redirect: false,
    ca: { trusted_system: true },
    ...overrides,
  } as StatusReport;
}

/** A default mock: the given proxies/rules/sites, `ok` for every mutation, and a
 *  reject for anything unexpected so a wrong wire call is loud. */
function stubIpc(opts: {
  proxies?: ProxyEntry[];
  rules?: ProxyRuleEntry[];
  sites?: SiteEntry[];
}) {
  const proxies = opts.proxies ?? [];
  const rules = opts.rules ?? [];
  const sites = opts.sites ?? [];
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_proxies":
        return Promise.resolve({ type: "proxies", proxies, rules });
      case "list_sites":
        return Promise.resolve({ type: "sites", sites });
      case "list_parked":
        return Promise.resolve({ type: "parked", paths: [] });
      case "list_groups":
        return Promise.resolve({ type: "groups", order: [], members: {} });
      case "add_proxy":
      case "remove_proxy":
      case "add_proxy_rule":
      case "remove_proxy_rule":
      case "set_secure":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

async function mountView() {
  const wrapper = mount(ProxiesView, {
    global: { stubs: { teleport: true, RouterLink: true } },
  });
  await flushPromises();
  return wrapper;
}

function clickByText(wrapper: Awaited<ReturnType<typeof mountView>>, text: string) {
  const btn = wrapper
    .findAll("button")
    .find((b) => b.text().includes(text) && b.attributes("disabled") === undefined);
  if (!btn) throw new Error(`no enabled button with text "${text}"`);
  return btn.trigger("click");
}

/** The most recent invoke call for `cmd`, or undefined. */
function lastCall(cmd: string): Record<string, unknown> | undefined {
  const calls = invokeMock.mock.calls.filter((c) => c[0] === cmd);
  const last = calls[calls.length - 1];
  return last?.[1] as Record<string, unknown> | undefined;
}

describe("ProxiesView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    openUrlMock.mockReset();
    resetResourceCache();
    // The daemon store is a module singleton; reset it so a report set by one
    // test doesn't leak into the next (which would flip unbound state).
    useDaemon().report.value = null;
  });

  it("renders whole-host proxies and path rules", async () => {
    stubIpc({
      proxies: [{ name: "reverb", target: "http://localhost:9011", secure: false }],
      rules: [{ site: "app", prefix: "/app", target: "http://localhost:8080" }],
    });
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("reverb.test");
    expect(wrapper.text()).toContain("http://localhost:9011");
    expect(wrapper.text()).toContain("/app");
    expect(wrapper.text()).toContain("http://localhost:8080");
  });

  it("shows the empty state when there are no proxies or rules", async () => {
    stubIpc({});
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("No proxies yet");
  });

  it("creates a whole-host proxy, normalizing the upstream URL", async () => {
    stubIpc({ sites: [site("app")] });
    const wrapper = await mountView();

    await clickByText(wrapper, "New proxy");
    await flushPromises();

    await wrapper.find("#proxyname").setValue("mydockersite");
    await wrapper.find("#proxyurl").setValue("http://localhost:9011/");
    await clickByText(wrapper, "Add proxy");
    await flushPromises();

    expect(lastCall("add_proxy")).toEqual({ name: "mydockersite", url: "http://localhost:9011" });
    expect(invokeMock.mock.calls.some((c) => c[0] === "set_secure")).toBe(false);
  });

  it("prepends http:// when the scheme is omitted", async () => {
    stubIpc({});
    const wrapper = await mountView();
    await clickByText(wrapper, "New proxy");
    await flushPromises();
    await wrapper.find("#proxyname").setValue("bare");
    await wrapper.find("#proxyurl").setValue("localhost:9011");
    await clickByText(wrapper, "Add proxy");
    await flushPromises();
    expect(lastCall("add_proxy")).toEqual({ name: "bare", url: "http://localhost:9011" });
  });

  it("enables HTTPS after creating when the switch is on", async () => {
    stubIpc({});
    const wrapper = await mountView();
    await clickByText(wrapper, "New proxy");
    await flushPromises();

    await wrapper.find("#proxyname").setValue("secureproxy");
    await wrapper.find("#proxyurl").setValue("http://localhost:9011");
    await wrapper.find('button[aria-label="HTTPS"]').trigger("click");
    await clickByText(wrapper, "Add proxy");
    await flushPromises();

    expect(lastCall("add_proxy")).toEqual({ name: "secureproxy", url: "http://localhost:9011" });
    expect(lastCall("set_secure")).toEqual({ name: "secureproxy", secure: true });
    // set_secure must run after add_proxy: the proxy has to exist before it can
    // be secured (the daemon starts new proxies on HTTP).
    const names = invokeMock.mock.calls.map((c) => c[0]);
    expect(names.indexOf("add_proxy")).toBeLessThan(names.indexOf("set_secure"));
  });

  it("adds a path rule to a site", async () => {
    stubIpc({ sites: [site("app")] });
    const wrapper = await mountView();

    await clickByText(wrapper, "New path rule");
    await flushPromises();

    await wrapper.find("#ruleprefix").setValue("/app");
    await wrapper.find("#ruleurl").setValue("http://localhost:9011");
    await clickByText(wrapper, "Add rule");
    await flushPromises();

    expect(lastCall("add_proxy_rule")).toEqual({
      site: "app",
      prefix: "/app",
      url: "http://localhost:9011",
    });
  });

  it("toggles HTTPS on an existing proxy via set_secure", async () => {
    stubIpc({ proxies: [{ name: "reverb", target: "http://localhost:9011", secure: false }] });
    const wrapper = await mountView();

    await clickByText(wrapper, "HTTP");
    await flushPromises();

    expect(lastCall("set_secure")).toEqual({ name: "reverb", secure: true });
  });

  it("removes a path rule", async () => {
    stubIpc({ rules: [{ site: "app", prefix: "/app", target: "http://localhost:9011" }] });
    const wrapper = await mountView();

    const trash = wrapper
      .findAll("button")
      .find((b) => b.attributes("aria-label")?.startsWith("Remove rule"));
    if (!trash) throw new Error("no remove-rule button");
    await trash.trigger("click");
    await flushPromises();

    await clickByText(wrapper, "Remove");
    await flushPromises();

    expect(lastCall("remove_proxy_rule")).toEqual({ site: "app", prefix: "/app" });
  });

  it("opens a proxy's domain in the browser when DNS is bound", async () => {
    stubIpc({ proxies: [{ name: "reverb", target: "http://localhost:9011", secure: false }] });
    useDaemon().report.value = boundReport();
    const wrapper = await mountView();

    const domain = wrapper.findAll("button").find((b) => b.text() === "reverb.test");
    if (!domain) throw new Error("no domain button");
    expect(domain.attributes("disabled")).toBeUndefined();
    await domain.trigger("click");
    await flushPromises();

    expect(openUrlMock).toHaveBeenCalledTimes(1);
    expect(openUrlMock).toHaveBeenCalledWith("http://reverb.test");
  });

  it("disables the open affordance for a proxy in unbound (resolver-off) mode", async () => {
    stubIpc({ proxies: [{ name: "reverb", target: "http://localhost:9011", secure: false }] });
    useDaemon().report.value = boundReport({ resolver_installed: false });
    const wrapper = await mountView();

    const domain = wrapper.findAll("button").find((b) => b.text() === "reverb.test");
    if (!domain) throw new Error("no domain button");
    expect(domain.attributes("disabled")).toBeDefined();
    await domain.trigger("click");
    await flushPromises();
    expect(openUrlMock).not.toHaveBeenCalled();
  });

  it("removes a proxy", async () => {
    stubIpc({ proxies: [{ name: "reverb", target: "http://localhost:9011", secure: false }] });
    const wrapper = await mountView();

    const trash = wrapper
      .findAll("button")
      .find((b) => b.attributes("aria-label")?.startsWith("Remove proxy"));
    if (!trash) throw new Error("no remove-proxy button");
    await trash.trigger("click");
    await flushPromises();

    await clickByText(wrapper, "Remove");
    await flushPromises();

    expect(lastCall("remove_proxy")).toEqual({ name: "reverb" });
  });
});
