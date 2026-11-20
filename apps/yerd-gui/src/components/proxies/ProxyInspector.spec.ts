import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openInBrowser = vi.fn();
vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  openInBrowser: (...args: unknown[]) => openInBrowser(...args),
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError, info: vi.fn() }),
}));

import ProxyInspector from "./ProxyInspector.vue";
import type { ProxyEntry, StatusReport } from "@/ipc/types";
import type { ProxyItem } from "@/lib/proxies";

const clipboardWriteText = vi.fn();

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ca: { path: "", fingerprint: "", trusted_system: true },
    ...overrides,
  } as StatusReport;
}

function proxy(overrides: Partial<ProxyEntry> = {}): ProxyEntry {
  return {
    name: "mydocker",
    target: "http://localhost:9011",
    secure: true,
    primary_domain: "mydocker.test",
    domains: ["mydocker.test", "*.mydocker.test", "docker.test"],
    ...overrides,
  };
}

function proxyItem(p: ProxyEntry = proxy()): ProxyItem {
  return { kind: "proxy", key: `proxy:${p.name}`, proxy: p };
}

const ruleItem: ProxyItem = {
  kind: "rule",
  key: "rule:codestash/live",
  rule: { site: "codestash", prefix: "/live", target: "http://127.0.0.1:8081" },
};

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

function mountInspector(item: ProxyItem | null, props: Record<string, unknown> = {}) {
  const w = mount(ProxyInspector, {
    attachTo: document.body,
    props: { item, mode: "drawer", open: true, report: report(), tld: "test", ...props },
    global: { stubs: { teleport: true, DialogPortal: Pass } },
  });
  mounted.push(w);
  return w;
}

beforeEach(() => {
  vi.clearAllMocks();
  openInBrowser.mockResolvedValue(undefined);
  clipboardWriteText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

function button(w: VueWrapper, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

describe("ProxyInspector for a whole-domain proxy", () => {
  it("shows the host, subtitle and upstream with an Edit button on Forward to", () => {
    const w = mountInspector(proxyItem());
    expect(w.get('[data-testid="proxy-title"]').text()).toBe("mydocker.test");
    expect(w.get('[data-testid="proxy-subtitle"]').text()).toBe(
      "Whole-domain proxy · every request goes to the upstream",
    );
    expect(w.get('[data-testid="forward-to"]').text()).toBe("http://localhost:9011");
    expect(w.get('[data-testid="edit-upstream"]').text()).toBe("Edit");
  });

  it("emits edit from Edit, and disables it while busy", async () => {
    const item = proxyItem();
    const w = mountInspector(item);
    await w.get('[data-testid="edit-upstream"]').trigger("click");
    expect(w.emitted("edit")).toEqual([[item]]);
    await w.setProps({ busy: true });
    expect(w.get('[data-testid="edit-upstream"]').attributes("disabled")).toBeDefined();
  });

  it("opens and copies the proxy URL", async () => {
    const w = mountInspector(proxyItem());
    await button(w, "Open").trigger("click");
    await button(w, "Copy URL").trigger("click");
    await flushPromises();
    expect(openInBrowser).toHaveBeenCalledWith("https://mydocker.test");
    expect(clipboardWriteText).toHaveBeenCalledWith("https://mydocker.test");
    expect(toastSuccess).toHaveBeenCalledWith("Copied URL", undefined);
  });

  it("disables Open while unbound", () => {
    const w = mountInspector(proxyItem(), { report: report({ resolver_installed: false }) });
    const open = button(w, "Open");
    expect(open.attributes("disabled")).toBeDefined();
    expect(open.attributes("title")).toBe(
      "mydocker.test is reachable only when .test DNS resolution is active",
    );
  });

  it.each([
    [true, "Certificate issued by Yerd local CA · trusted"],
    [false, "Certificate issued by Yerd local CA · not trusted"],
    [null, "Certificate issued by Yerd local CA · trust unknown"],
  ] as const)("words HTTPS for trust %s", (trusted, text) => {
    const w = mountInspector(proxyItem(), {
      report: report({ ca: { path: "", fingerprint: "", trusted_system: trusted } }),
    });
    expect(w.get('[data-testid="https-subtitle"]').text()).toBe(text);
  });

  it("emits toggleSecure, and not while busy", async () => {
    const p = proxy();
    const w = mountInspector(proxyItem(p));
    await w.get('button[aria-label="HTTPS"]').trigger("click");
    expect(w.emitted("toggleSecure")).toEqual([[p]]);
    await w.setProps({ busy: true });
    expect(w.get('button[aria-label="HTTPS"]').attributes("disabled")).toBeDefined();
    expect(w.get('[data-testid="inspector-footer-action"]').attributes("disabled")).toBeDefined();
  });

  it("tags the domains and forwards a card change", async () => {
    const w = mountInspector(proxyItem());
    const card = w.get('[data-testid="domains-card"]');
    expect(card.text()).toContain("primary");
    expect(card.text()).toContain("wildcard");
    expect(card.text()).toContain("alias");
    await button(w, "Add domain").trigger("click");
    await flushPromises();
    const domains = w.findComponent({ name: "SiteDomainsCard" });
    expect(domains.findComponent({ name: "DomainDialog" }).props("open")).toBe(true);
    domains.vm.$emit("domainsChanged");
    expect(w.emitted("domainsChanged")).toHaveLength(1);
  });

  it("emits remove from Remove proxy…", async () => {
    const item = proxyItem();
    const w = mountInspector(item);
    const action = w.get('[data-testid="inspector-footer-action"]');
    expect(action.text()).toBe("Remove proxy…");
    await action.trigger("click");
    expect(w.emitted("remove")).toEqual([[item]]);
  });
});

describe("ProxyInspector for a path rule", () => {
  it("shows only the title, subtitle, upstream and Remove path…", async () => {
    const w = mountInspector(ruleItem);
    expect(w.get('[data-testid="proxy-title"]').text()).toBe("codestash.test/live");
    expect(w.get('[data-testid="proxy-subtitle"]').text()).toBe(
      "Path on a site · PHP serves the rest of codestash.test",
    );
    expect(w.get('[data-testid="forward-to"]').text()).toBe("http://127.0.0.1:8081");
    expect(w.findAll("button").some((b) => ["Open", "Copy URL"].includes(b.text().trim()))).toBe(false);
    expect(w.find('button[aria-label="HTTPS"]').exists()).toBe(false);
    expect(w.find('[data-testid="domains-card"]').exists()).toBe(false);
    const action = w.get('[data-testid="inspector-footer-action"]');
    expect(action.text()).toBe("Remove path…");
    await action.trigger("click");
    expect(w.emitted("remove")).toEqual([[ruleItem]]);
  });

  it("emits edit from Edit on Forward to", async () => {
    const w = mountInspector(ruleItem);
    await w.get('[data-testid="edit-upstream"]').trigger("click");
    expect(w.emitted("edit")).toEqual([[ruleItem]]);
    expect(w.find('[data-testid="managed-by"]').exists()).toBe(false);
  });

  it("offers no Edit for a service-managed rule and says which service owns it", () => {
    const managed: ProxyItem = {
      kind: "rule",
      key: "rule:codestash/app",
      rule: { site: "codestash", prefix: "/app", target: "http://127.0.0.1:8080", managed_by: "Reverb" },
    };
    const w = mountInspector(managed);
    expect(w.find('[data-testid="edit-upstream"]').exists()).toBe(false);
    expect(w.get('[data-testid="managed-by"]').text()).toBe("Managed by Reverb");
    expect(w.get('[data-testid="forward-to"]').text()).toBe("http://127.0.0.1:8080");
    expect(w.get('[data-testid="inspector-footer-action"]').text()).toBe("Remove path…");
  });
});

describe("ProxyInspector drawer", () => {
  it("closes on Escape, the backdrop and its X", async () => {
    const w = mountInspector(proxyItem());
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await w.get(".proxy-inspector-backdrop").trigger("click");
    await w.get('[aria-label="Close proxy details"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(3);
  });

  it("renders nothing while closed and no drawer chrome in panel mode", () => {
    const closed = mountInspector(proxyItem(), { open: false });
    expect(closed.find('[data-testid="proxy-inspector"]').exists()).toBe(false);
    const panel = mountInspector(proxyItem(), { mode: "panel" });
    expect(panel.find(".proxy-inspector-backdrop").exists()).toBe(false);
    expect(panel.get('[data-testid="proxy-inspector"]').attributes("role")).toBeUndefined();
  });
});
