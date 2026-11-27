import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const openUrlMock = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => openUrlMock(...args),
}));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError, info: vi.fn() }),
}));

import ProxiesView from "./ProxiesView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { resetResourceCache } from "@/composables/useResource";
import type { ProxyEntry, ProxyRuleEntry, SiteEntry, StatusReport } from "@/ipc/types";

function site(name: string, secure = false): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.3",
    secure,
    kind: "linked",
    is_wordpress: false,
  };
}

/** A minimal bound, resolver-installed daemon report: enough for the URL
 *  helpers, so a proxy's Open action is enabled (not unbound). */
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
 *  reject for anything unexpected so a wrong wire call is loud. `failures` makes
 *  the named commands reject. */
function stubIpc(opts: {
  proxies?: ProxyEntry[];
  rules?: ProxyRuleEntry[];
  sites?: SiteEntry[];
  failures?: string[];
  commands?: Record<string, (args?: Record<string, unknown>) => Promise<unknown>>;
}) {
  const proxies = opts.proxies ?? [];
  const rules = opts.rules ?? [];
  const sites = opts.sites ?? [];
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (opts.failures?.includes(cmd)) return Promise.reject(new Error(`${cmd} failed`));
    const override = opts.commands?.[cmd];
    if (override) return override(args);
    switch (cmd) {
      case "list_proxies":
        return Promise.resolve({ type: "proxies", proxies, rules });
      case "list_sites":
        return Promise.resolve({ type: "sites", sites });
      case "list_parked":
        return Promise.resolve({ type: "parked", paths: [] });
      case "list_groups":
        return Promise.resolve({ type: "groups", order: [], members: {} });
      case "list_response_headers":
        return Promise.resolve({ type: "response_headers", headers: [] });
      case "add_proxy":
      case "remove_proxy":
      case "add_proxy_rule":
      case "remove_proxy_rule":
      case "update_proxy":
      case "update_proxy_rule":
      case "set_secure":
      case "add_domain":
      case "remove_domain":
      case "set_primary_domain":
      case "reset_domains":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

function setWide(wide: boolean | null): void {
  globalThis.matchMedia = (
    wide === null
      ? undefined
      : vi.fn(() => ({ matches: wide, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
  ) as unknown as typeof globalThis.matchMedia;
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(ProxiesView, {
    attachTo: document.body,
    global: {
      stubs: { teleport: true, RouterLink: true, DialogPortal: Pass, AlertDialogPortal: Pass },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type View = Awaited<ReturnType<typeof mountView>>;

function button(w: View, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

async function click(w: View, text: string) {
  await button(w, text).trigger("click");
  await flushPromises();
}

function row(w: View, key: string) {
  return w.get(`[data-key="${key}"]`);
}

async function openRow(w: View, key: string) {
  await row(w, key).trigger("click");
  await flushPromises();
  return w.get('[data-testid="proxy-inspector"]');
}

async function openNew(w: View) {
  await click(w, "New proxy");
}

/** The most recent invoke call for `cmd`, or undefined. */
function lastCall(cmd: string): Record<string, unknown> | undefined {
  const calls = invokeMock.mock.calls.filter((c) => c[0] === cmd);
  const last = calls[calls.length - 1];
  return last?.[1] as Record<string, unknown> | undefined;
}

function called(cmd: string): boolean {
  return invokeMock.mock.calls.some((c) => c[0] === cmd);
}

const reverb = (): ProxyEntry => ({ name: "reverb", target: "http://localhost:9011", secure: false });
const appRule = (): ProxyRuleEntry => ({ site: "app", prefix: "/app", target: "http://localhost:8080" });
const dotted = (): ProxyEntry => ({
  name: "api.account",
  target: "http://127.0.0.1:9011",
  secure: false,
  primary_domain: "custom-domain.test",
  domains: ["api.account.test", "custom-domain.test"],
});

beforeEach(() => {
  invokeMock.mockReset();
  openUrlMock.mockReset();
  push.mockReset();
  toastSuccess.mockReset();
  toastError.mockReset();
  resetResourceCache();
  useDaemon().report.value = null;
  setWide(null);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("ProxiesView list", () => {
  it("renders whole-domain proxies and path rules in their sections", async () => {
    stubIpc({ proxies: [reverb()], rules: [appRule()] });
    const w = await mountView();
    const sections = w.findAll("[data-section]");
    expect(sections.map((s) => s.attributes("data-section"))).toEqual(["proxy", "rule"]);
    expect(sections[0]?.text()).toContain("Whole domains");
    expect(sections[0]?.text()).toContain("reverb.test");
    expect(sections[0]?.text()).toContain("localhost:9011");
    expect(sections[1]?.text()).toContain("Paths on sites");
    expect(sections[1]?.text()).toContain("app.test/app");
    expect(sections[1]?.text()).toContain("localhost:8080");
  });

  it("counts the chips and filters the sections", async () => {
    stubIpc({ proxies: [reverb(), dotted()], rules: [appRule()] });
    const w = await mountView();
    const chip = (key: string) => w.get(`button[data-chip="${key}"]`);
    expect(chip("all").text()).toBe("All 3");
    expect(chip("domains").text()).toBe("Domains 2");
    expect(chip("paths").text()).toBe("Paths 1");
    await chip("paths").trigger("click");
    expect(w.findAll("[data-section]").map((s) => s.attributes("data-section"))).toEqual(["rule"]);
    await chip("domains").trigger("click");
    expect(w.findAll("[data-section]").map((s) => s.attributes("data-section"))).toEqual(["proxy"]);
  });

  it("orders proxies by name and paths by site then prefix", async () => {
    stubIpc({
      proxies: [reverb(), dotted()],
      rules: [appRule(), { site: "app", prefix: "/api", target: "http://x:1" }, { site: "a", prefix: "/z", target: "http://x:2" }],
    });
    const w = await mountView();
    expect(w.findAll("[data-key]").map((r) => r.attributes("data-key"))).toEqual([
      "proxy:api.account",
      "proxy:reverb",
      "rule:a/z",
      "rule:app/api",
      "rule:app/app",
    ]);
  });

  it("shows extra domains in the table and the alias count in the wide list", async () => {
    stubIpc({ proxies: [dotted(), reverb()] });
    const compact = await mountView();
    expect(row(compact, "proxy:api.account").get('[data-testid="row-domains"]').text()).toBe("api.account.test");
    expect(row(compact, "proxy:reverb").get('[data-testid="row-domains"]').text()).toBe("—");

    setWide(true);
    const wide = await mountView();
    expect(row(wide, "proxy:api.account").get('[data-testid="row-aliases"]').text()).toBe("+1");
    expect(row(wide, "proxy:reverb").find('[data-testid="row-aliases"]').exists()).toBe(false);
  });

  it("shows the empty state, whose tiles open New proxy in their mode", async () => {
    stubIpc({ sites: [site("app")] });
    const w = await mountView();
    expect(w.text()).toContain("No proxies yet");
    await w.get('[data-testid="empty-path"]').trigger("click");
    await flushPromises();
    expect(w.get('[data-mode="path"]').attributes("data-state")).toBe("checked");
    expect(w.find("#proxy-prefix").exists()).toBe(true);
  });

  it("disables the path tile without sites", async () => {
    stubIpc({});
    const w = await mountView();
    expect(w.get('[data-testid="empty-path"]').attributes("disabled")).toBeDefined();
  });
});

describe("ProxiesView layouts", () => {
  it("selects the first proxy in the wide layout", async () => {
    setWide(true);
    stubIpc({ proxies: [reverb()], rules: [appRule()] });
    const w = await mountView();
    expect(w.get('[data-layout]').attributes("data-layout")).toBe("wide");
    expect(row(w, "proxy:reverb").attributes("aria-selected")).toBe("true");
    expect(w.get('[data-testid="proxy-title"]').text()).toBe("reverb.test");
  });

  it("opens the drawer from a compact row and closes it when the item vanishes", async () => {
    let proxies = [reverb()];
    stubIpc({ proxies });
    const w = await mountView();
    expect(w.find('[data-testid="proxy-inspector"]').exists()).toBe(false);
    const drawer = await openRow(w, "proxy:reverb");
    expect(drawer.attributes("role")).toBe("dialog");
    expect(drawer.get('[data-testid="proxy-title"]').text()).toBe("reverb.test");

    proxies = [];
    stubIpc({ proxies, rules: [appRule()] });
    await w.get('[data-testid="proxy-inspector"] [data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    await click(w, "Remove proxy");
    expect(w.find('[data-testid="proxy-inspector"]').exists()).toBe(false);
  });

  it("does not paint the selected compact row outside keyboard focus", async () => {
    stubIpc({ proxies: [reverb()] });
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    const classes = row(w, "proxy:reverb").classes();
    expect(classes).toContain("group-focus-visible/list:bg-brand/25");
    expect(classes).not.toContain("bg-brand/[0.14]");
  });
});

describe("ProxiesView shadowed proxy", () => {
  it.each([
    ["a same-named site hides the headers card", ["reverb"], false],
    ["an unrelated site keeps it", ["app"], true],
  ])("%s", async (_label, siteNames, shown) => {
    stubIpc({ proxies: [reverb()], sites: siteNames.map((n) => site(n)) });
    const w = await mountView();
    const inspector = await openRow(w, "proxy:reverb");
    expect(inspector.find('[data-testid="headers-card"]').exists()).toBe(shown);
    expect(inspector.find('[data-testid="headers-shadowed"]').exists()).toBe(!shown);
  });

  it("keeps the headers card hidden while the site list can't load", async () => {
    stubIpc({ proxies: [reverb()], failures: ["list_sites"] });
    const w = await mountView();
    const inspector = await openRow(w, "proxy:reverb");
    expect(inspector.find('[data-testid="headers-card"]').exists()).toBe(false);
    expect(inspector.find('[data-testid="headers-shadowed"]').exists()).toBe(false);
  });
});

describe("ProxiesView CA strip", () => {
  it.each([
    ["a secure proxy and an untrusted CA", true, false, true],
    ["an unknown trust", true, null, false],
    ["a trusted CA", true, true, false],
    ["no secure proxy", false, false, false],
  ] as const)("with %s shows the strip: %s", async (_label, secure, trusted, shown) => {
    stubIpc({ proxies: [{ ...reverb(), secure }] });
    useDaemon().report.value = boundReport({ ca: { trusted_system: trusted } } as Partial<StatusReport>);
    const w = await mountView();
    expect(w.find('[data-warning="ca"]').exists()).toBe(shown);
  });

  it("routes Trust now to Doctor", async () => {
    stubIpc({ proxies: [{ ...reverb(), secure: true }] });
    useDaemon().report.value = boundReport({ ca: { trusted_system: false } } as Partial<StatusReport>);
    const w = await mountView();
    await w.get('[data-warning="ca"] button').trigger("click");
    expect(push).toHaveBeenCalledWith("/doctor");
  });
});

describe("ProxiesView new proxy", () => {
  it("creates a whole-domain proxy, normalising the upstream URL", async () => {
    stubIpc({ sites: [site("app")] });
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-name").setValue("mydockersite");
    await w.get("#proxy-url").setValue("http://localhost:9011/");
    await click(w, "Add proxy");
    expect(lastCall("add_proxy")).toEqual({ name: "mydockersite", url: "http://localhost:9011" });
    expect(called("set_secure")).toBe(false);
    expect(toastSuccess).toHaveBeenCalledWith("Added mydockersite.test", "Forwarding to localhost:9011");
  });

  it("prepends http:// when the scheme is omitted", async () => {
    stubIpc({});
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-name").setValue("bare");
    await w.get("#proxy-url").setValue("localhost:9011");
    await click(w, "Add proxy");
    expect(lastCall("add_proxy")).toEqual({ name: "bare", url: "http://localhost:9011" });
  });

  it("enables HTTPS after creating when the switch is on", async () => {
    stubIpc({});
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-name").setValue("secureproxy");
    await w.get("#proxy-url").setValue("http://localhost:9011");
    await w.get('[role="dialog"] button[aria-label="HTTPS"]').trigger("click");
    await click(w, "Add proxy");
    expect(lastCall("add_proxy")).toEqual({ name: "secureproxy", url: "http://localhost:9011" });
    expect(lastCall("set_secure")).toEqual({ name: "secureproxy", secure: true });
    const names = invokeMock.mock.calls.map((c) => c[0]);
    expect(names.indexOf("add_proxy")).toBeLessThan(names.indexOf("set_secure"));
  });

  it("reports a proxy added on HTTP only when HTTPS fails", async () => {
    stubIpc({ failures: ["set_secure"] });
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-name").setValue("grafana");
    await w.get("#proxy-url").setValue("localhost:3000");
    await w.get('[role="dialog"] button[aria-label="HTTPS"]').trigger("click");
    await click(w, "Add proxy");
    expect(toastError).toHaveBeenCalledWith(
      "Added grafana.test on HTTP only",
      "Couldn't enable HTTPS: set_secure failed",
    );
  });

  it("accepts a dotted name and drops a typed .test suffix", async () => {
    stubIpc({});
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-name").setValue("api.account.test");
    await w.get("#proxy-url").setValue("http://127.0.0.1:9011");
    await click(w, "Add proxy");
    expect(lastCall("add_proxy")).toEqual({ name: "api.account", url: "http://127.0.0.1:9011" });
  });

  it("keeps Add proxy disabled for a malformed name", async () => {
    stubIpc({});
    const w = await mountView();
    await openNew(w);
    await w.get("#proxy-url").setValue("http://127.0.0.1:9011");
    const disabled = () => button(w, "Add proxy").attributes("disabled") !== undefined;
    for (const bad of ["api..account", "api account"]) {
      await w.get("#proxy-name").setValue(bad);
      expect(disabled()).toBe(true);
    }
    await w.get("#proxy-name").setValue("api.account");
    expect(disabled()).toBe(false);
  });

  it("adds a path on a site", async () => {
    stubIpc({ sites: [site("app")] });
    const w = await mountView();
    await openNew(w);
    await w.get('[data-mode="path"]').trigger("click");
    await flushPromises();
    await w.get("#proxy-prefix").setValue("/app");
    await w.get("#proxy-url").setValue("http://localhost:9011");
    await click(w, "Add path");
    expect(lastCall("add_proxy_rule")).toEqual({ site: "app", prefix: "/app", url: "http://localhost:9011" });
    expect(toastSuccess).toHaveBeenCalledWith("Added app.test/app", "Forwarding to localhost:9011");
  });
});

describe("ProxiesView inspector", () => {
  it("toggles HTTPS on an existing proxy via set_secure", async () => {
    stubIpc({ proxies: [reverb()] });
    const w = await mountView();
    const drawer = await openRow(w, "proxy:reverb");
    await drawer.get('button[aria-label="HTTPS"]').trigger("click");
    await flushPromises();
    expect(lastCall("set_secure")).toEqual({ name: "reverb", secure: true });
  });

  it("opens a proxy in the browser when DNS is bound", async () => {
    stubIpc({ proxies: [reverb()] });
    useDaemon().report.value = boundReport();
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    await click(w, "Open");
    expect(openUrlMock).toHaveBeenCalledTimes(1);
    expect(openUrlMock).toHaveBeenCalledWith("http://reverb.test");
  });

  it("disables Open in unbound (resolver-off) mode", async () => {
    stubIpc({ proxies: [reverb()] });
    useDaemon().report.value = boundReport({ resolver_installed: false });
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    const open = button(w, "Open");
    expect(open.attributes("disabled")).toBeDefined();
    expect(open.attributes("title")).toBe("reverb.test is reachable only when .test DNS resolution is active");
    await open.trigger("click");
    await flushPromises();
    expect(openUrlMock).not.toHaveBeenCalled();
  });

  it("opens a customised proxy at its primary domain", async () => {
    stubIpc({ proxies: [dotted()] });
    useDaemon().report.value = boundReport();
    const w = await mountView();
    const drawer = await openRow(w, "proxy:api.account");
    expect(drawer.get('[data-testid="proxy-title"]').text()).toBe("custom-domain.test");
    await click(w, "Open");
    expect(openUrlMock).toHaveBeenCalledWith("http://custom-domain.test");
  });

  it("removes a proxy after confirming, and not on Cancel", async () => {
    stubIpc({ proxies: [reverb()] });
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    await w.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    const dialog = w.get('[role="alertdialog"]');
    expect(dialog.text()).toContain("Remove reverb.test?");
    expect(dialog.text()).toContain("Whatever runs on localhost:9011 keeps running.");
    await click(w, "Cancel");
    expect(called("remove_proxy")).toBe(false);

    await w.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    await click(w, "Remove proxy");
    expect(lastCall("remove_proxy")).toEqual({ name: "reverb" });
  });

  it("keeps a second remove confirm intact when the first removal finishes", async () => {
    let finish = (): void => {};
    stubIpc({
      proxies: [reverb(), { ...dotted(), primary_domain: undefined, domains: undefined }],
      commands: {
        remove_proxy: (args) =>
          args?.name === "reverb"
            ? new Promise((resolve) => {
                finish = () => resolve({ type: "ok" });
              })
            : Promise.resolve({ type: "ok" }),
      },
    });
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    await w.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    await click(w, "Remove proxy");

    await openRow(w, "proxy:api.account");
    await w.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    finish();
    await flushPromises();
    expect(w.get('[role="alertdialog"]').text()).toContain("Remove api.account.test?");
    await click(w, "Remove proxy");
    expect(lastCall("remove_proxy")).toEqual({ name: "api.account" });
  });

  it("removes a path after confirming", async () => {
    stubIpc({ rules: [appRule()] });
    const w = await mountView();
    const drawer = await openRow(w, "rule:app/app");
    expect(drawer.text()).toContain("Path on a site · PHP serves the rest of app.test");
    expect(drawer.find('[data-testid="domains-card"]').exists()).toBe(false);
    await drawer.get('[data-testid="inspector-footer-action"]').trigger("click");
    await flushPromises();
    expect(w.get('[role="alertdialog"]').text()).toContain("Remove app.test/app?");
    await click(w, "Remove path");
    expect(lastCall("remove_proxy_rule")).toEqual({ site: "app", prefix: "/app" });
  });

  it("manages a proxy's domains through the card and reloads on change", async () => {
    stubIpc({ proxies: [dotted()] });
    const w = await mountView();
    const drawer = await openRow(w, "proxy:api.account");
    await button(w, "Add domain").trigger("click");
    await flushPromises();
    expect(drawer.text()).toContain("custom-domain.test");

    const before = invokeMock.mock.calls.filter((c) => c[0] === "list_proxies").length;
    w.findComponent({ name: "DomainDialog" }).vm.$emit("submit", "extra.test");
    await flushPromises();
    expect(lastCall("add_domain")).toEqual({ name: "api.account", domain: "extra.test" });
    const after = invokeMock.mock.calls.filter((c) => c[0] === "list_proxies").length;
    expect(after).toBeGreaterThan(before);
  });
});

describe("ProxiesView edit upstream", () => {
  async function editUpstream(w: View, key: string, typed: string) {
    await openRow(w, key);
    await w.get('[data-testid="edit-upstream"]').trigger("click");
    await flushPromises();
    await w.get("#edit-upstream-url").setValue(typed);
    await click(w, "Save");
  }

  it("edits a whole-domain proxy's upstream", async () => {
    stubIpc({ proxies: [reverb()] });
    const w = await mountView();
    await openRow(w, "proxy:reverb");
    await w.get('[data-testid="edit-upstream"]').trigger("click");
    await flushPromises();
    expect((w.get("#edit-upstream-url").element as HTMLInputElement).value).toBe("localhost:9011");
    const before = invokeMock.mock.calls.filter((c) => c[0] === "list_proxies").length;
    await w.get("#edit-upstream-url").setValue("3001");
    await click(w, "Save");
    expect(lastCall("update_proxy")).toEqual({ name: "reverb", url: "http://127.0.0.1:3001" });
    expect(toastSuccess).toHaveBeenCalledWith("Updated reverb.test", "Forwarding to 127.0.0.1:3001");
    const after = invokeMock.mock.calls.filter((c) => c[0] === "list_proxies").length;
    expect(after).toBeGreaterThan(before);
  });

  it("edits a path rule's upstream", async () => {
    stubIpc({ rules: [appRule()] });
    const w = await mountView();
    await editUpstream(w, "rule:app/app", "127.0.0.1:8082");
    expect(lastCall("update_proxy_rule")).toEqual({
      site: "app",
      prefix: "/app",
      url: "http://127.0.0.1:8082",
    });
    expect(toastSuccess).toHaveBeenCalledWith("Updated app.test/app", "Forwarding to 127.0.0.1:8082");
  });

  it("shows an error toast when the update fails", async () => {
    stubIpc({ proxies: [reverb()], failures: ["update_proxy"] });
    const w = await mountView();
    await editUpstream(w, "proxy:reverb", "3001");
    expect(toastError).toHaveBeenCalledWith("Couldn't update proxy", "update_proxy failed");
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  it("shows an error toast when a path update fails", async () => {
    stubIpc({ rules: [appRule()], failures: ["update_proxy_rule"] });
    const w = await mountView();
    await editUpstream(w, "rule:app/app", "3001");
    expect(toastError).toHaveBeenCalledWith("Couldn't update path", "update_proxy_rule failed");
  });
});
