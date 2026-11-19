import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
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
import { useOperations } from "@/composables/useOperations";
import { resetResourceCache } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import type { CloudflaredStatus, SiteEntry, SiteHostname, StatusReport, TunnelInfo } from "@/ipc/types";

function site(name: string): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.3",
    secure: true,
    kind: "linked",
    is_wordpress: false,
  };
}

const MANAGED: CloudflaredStatus = { installed: true, version: "2026.6.1", source: "managed", logged_in: true };

interface StubOpts {
  tunnels?: TunnelInfo[];
  named?: SiteHostname[];
  cloudflared?: CloudflaredStatus;
  tunnelStatus?: () => Promise<unknown>;
  routeDns?: () => Promise<unknown>;
  jobStates?: ("running" | "succeeded")[];
}

/** A default mock: the given tunnels and named sites, `ok` for every mutation, and
 *  a reject for anything unexpected so a wrong wire call is loud. */
function stubIpc(opts: StubOpts = {}) {
  const tunnels = opts.tunnels ?? [];
  const named = opts.named ?? [];
  const cloudflared = opts.cloudflared ?? MANAGED;
  const jobStates = [...(opts.jobStates ?? ["succeeded"])];
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "status":
        return Promise.resolve({ type: "status", report: { tld: "test" } as StatusReport });
      case "tunnel_status":
        return opts.tunnelStatus?.() ?? Promise.resolve({ type: "tunnels", tunnels, cloudflared });
      case "list_sites":
        return Promise.resolve({ type: "sites", sites: [site("app"), site("blog")] });
      case "list_named_tunnels":
        return Promise.resolve({
          type: "named_tunnels",
          tunnels: [{ name: "dev", uuid: "0000-1111" }],
          sites: named,
          zone: "example.com",
        });
      case "start_named_tunnel":
        return Promise.resolve({ type: "tunnels", tunnels, cloudflared });
      case "start_quick_tunnel":
        return Promise.resolve({
          type: "tunnels",
          tunnels: [{ site: args?.site, kind: "quick", state: "running", url: "https://x.trycloudflare.com" }],
          cloudflared,
        });
      case "stop_tunnel":
        return Promise.resolve({ type: "tunnels", tunnels: [], cloudflared });
      case "install_cloudflared_streamed":
      case "cloudflared_login":
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      case "job_status":
        return Promise.resolve({
          type: "job_progress",
          state: jobStates.shift() ?? "succeeded",
          phase: "",
          log: ["working"],
          next_cursor: 1,
          error: null,
        });
      case "route_tunnel_dns":
        return opts.routeDns?.() ?? Promise.resolve({ type: "ok" });
      case "set_site_tunnel":
      case "set_tunnel_password":
      case "create_named_tunnel":
      case "delete_named_tunnel":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

function setWide(wide: boolean): void {
  globalThis.matchMedia = vi.fn(() => ({
    matches: wide,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof globalThis.matchMedia;
}

const Pass = { template: "<div><slot /></div>" };
const ComboboxStub = {
  name: "Combobox",
  props: ["modelValue", "options", "disabled"],
  emits: ["update:modelValue"],
  template:
    '<div><button v-for="o in options" :key="o.value" type="button" :data-option="o.value" @click="$emit(\'update:modelValue\', o.value)">{{ o.label }}</button></div>',
};

const mounted: VueWrapper[] = [];

async function mountView() {
  await useDaemon().refresh();
  const wrapper = mount(IntegrationsView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: true,
        RouterLink: true,
        DialogPortal: Pass,
        AlertDialogPortal: Pass,
        Combobox: ComboboxStub,
      },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function buttons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll("button"));
}

function buttonByText(text: string): HTMLButtonElement {
  const btn = buttons().find((b) => b.textContent?.trim() === text && !b.disabled);
  if (!btn) throw new Error(`no enabled button "${text}"`);
  return btn;
}

async function click(el: Element | null): Promise<void> {
  if (!el) throw new Error("nothing to click");
  (el as HTMLElement).click();
  await flushPromises();
}

function calls(cmd: string): Record<string, unknown>[] {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1] as Record<string, unknown>);
}

function commands(): string[] {
  return invokeMock.mock.calls.map((c) => c[0] as string);
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

function dialogTitles(): string[] {
  return Array.from(document.querySelectorAll('[role="dialog"] h2, [role="alertdialog"] h2')).map(
    (h) => h.textContent?.trim() ?? "",
  );
}

function inspector(): Element | null {
  return document.querySelector('[data-testid="share-inspector"]');
}

async function openShareDialog(): Promise<void> {
  await click(document.querySelector('[data-testid="open-share"]'));
}

async function openSetup(): Promise<void> {
  await click(document.querySelector('[data-testid="open-setup"]'));
}

function named(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "@named", kind: "named", state, ...extra };
}

function quick(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "app", kind: "quick", state, url: "https://x.trycloudflare.com", ...extra };
}

beforeEach(() => {
  invokeMock.mockReset();
  resetResourceCache();
  useDaemon().report.value = null;
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  setWide(true);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("IntegrationsView list", () => {
  it("groups quick and domain shares and selects the first on a wide window", async () => {
    stubIpc({ tunnels: [quick("running"), named("running")], named: [{ site: "blog", hostname: "blog.example.com" }] });
    await mountView();
    const sections = Array.from(document.querySelectorAll("[data-section]"));
    expect(sections.map((s) => s.getAttribute("aria-label"))).toEqual(["Quick links", "Your domain · example.com"]);
    expect(document.querySelector('[aria-current="true"]')?.getAttribute("data-key")).toBe("quick:app");
    expect(inspector()?.textContent).toContain("Stop sharing");
  });

  it("opens the compact drawer on a row click and closes it", async () => {
    setWide(false);
    stubIpc({ tunnels: [quick("running")] });
    await mountView();
    expect(inspector()).toBeNull();
    await click(document.querySelector('[data-key="quick:app"]'));
    expect(inspector()).not.toBeNull();
    await click(document.querySelector('[aria-label="Close share details"]'));
    expect(inspector()).toBeNull();
  });

  it("shows the load error when nothing is cached", async () => {
    stubIpc({ tunnelStatus: () => Promise.reject(new Error("daemon unreachable")) });
    await mountView();
    expect(document.body.textContent).toContain("Couldn't load this");
    expect(document.body.textContent).toContain("daemon unreachable");
  });
});

describe("IntegrationsView empty states", () => {
  it("installs cloudflared from the not-installed state", async () => {
    stubIpc({ cloudflared: { installed: false, logged_in: false } });
    await mountView();
    expect(document.body.textContent).toContain("Sharing needs cloudflared");
    await click(buttonByText("Install cloudflared"));
    expect(calls("install_cloudflared_streamed")).toHaveLength(1);
  });

  it("opens Share a site from the nothing-shared state", async () => {
    stubIpc();
    await mountView();
    expect(document.body.textContent).toContain("Nothing shared yet");
    const empty = document.querySelector('[data-testid="share-empty"]');
    await click(empty?.querySelector("button") ?? null);
    expect(dialogTitles()).toContain("Share a site");
  });
});

describe("IntegrationsView sharing", () => {
  it("shares a quick link, toasts its URL and opens it in the compact drawer", async () => {
    setWide(false);
    stubIpc({ tunnels: [named("running")], named: [{ site: "blog", hostname: "blog.example.com" }] });
    await mountView();
    await openShareDialog();
    await click(document.querySelector('[data-option="app"]'));
    await click(document.querySelector('[data-testid="share-submit"]'));

    expect(calls("start_quick_tunnel")).toEqual([{ site: "app" }]);
    expect(useToast().toasts.value.find((t) => t.title === "Sharing app")?.detail).toBe(
      "https://x.trycloudflare.com",
    );
    expect(dialogTitles()).not.toContain("Share a site");
    expect(inspector()?.querySelector('[data-testid="share-chip"]')?.textContent?.trim()).toBe("Quick link");
  });

  it("exposes a site by saving the hostname then routing DNS, never starting the tunnel", async () => {
    stubIpc();
    await mountView();
    await openShareDialog();
    await click(document.querySelector('[data-kind="domain"]'));
    await click(document.querySelector('[data-option="blog"]'));
    await click(document.querySelector('[data-testid="share-submit"]'));

    expect(calls("set_site_tunnel")).toEqual([{ site: "blog", hostname: "blog.example.com" }]);
    expect(calls("route_tunnel_dns")).toEqual([{ tunnel: "dev", hostname: "blog.example.com" }]);
    const names = commands();
    expect(names.indexOf("set_site_tunnel")).toBeLessThan(names.indexOf("route_tunnel_dns"));
    expect(names).not.toContain("start_named_tunnel");
    expect(names).not.toContain("stop_named_tunnel");
  });

  it("rolls the hostname back when routing DNS fails and keeps the dialog open", async () => {
    stubIpc({ routeDns: () => Promise.reject(new Error("DNS record already exists")) });
    await mountView();
    await openShareDialog();
    await click(document.querySelector('[data-kind="domain"]'));
    await click(document.querySelector('[data-option="blog"]'));
    await click(document.querySelector('[data-testid="share-submit"]'));

    expect(calls("set_site_tunnel")).toEqual([
      { site: "blog", hostname: "blog.example.com" },
      { site: "blog", hostname: null },
    ]);
    expect(toastTitles()).toContain("Couldn't expose blog");
    expect(dialogTitles()).toContain("Share a site");
  });

  it("stops a quick link", async () => {
    stubIpc({ tunnels: [quick("running")] });
    await mountView();
    await click(inspector()?.querySelector('[data-action="stop"]') ?? null);
    expect(calls("stop_tunnel")).toEqual([{ site: "app" }]);
  });

  it("removing a site from the domain only clears its hostname", async () => {
    stubIpc({ tunnels: [named("running")], named: [{ site: "app", hostname: "app.example.com" }] });
    await mountView();
    await click(inspector()?.querySelector('[data-action="remove"]') ?? null);

    expect(calls("set_site_tunnel")).toEqual([{ site: "app", hostname: null }]);
    const names = commands();
    expect(names).not.toContain("route_tunnel_dns");
    expect(names).not.toContain("start_named_tunnel");
    expect(names).not.toContain("stop_named_tunnel");
  });

  it("restarts a failed domain tunnel from the inspector", async () => {
    stubIpc({ tunnels: [named("failed")], named: [{ site: "app", hostname: "app.example.com" }] });
    await mountView();
    await click(inspector()?.querySelector('[data-action="restart"]') ?? null);
    expect(calls("start_named_tunnel")).toHaveLength(1);
  });

  it("disables Share a site while an action runs", async () => {
    stubIpc({ tunnels: [quick("running")] });
    await mountView();
    const share = document.querySelector('[data-testid="open-share"]') as HTMLButtonElement;
    expect(share.disabled).toBe(false);
    useOperations().begin({ id: "cloudflared-install", kind: "tool-install", label: "Installing cloudflared" });
    await flushPromises();
    expect(share.disabled).toBe(true);
    useOperations().end("cloudflared-install");
  });
});

describe("IntegrationsView share password", () => {
  async function openPassword(): Promise<void> {
    await click(inspector()?.querySelector('[data-action="password"]') ?? null);
  }

  it("sets a share password", async () => {
    stubIpc({ tunnels: [quick("running")] });
    await mountView();
    await openPassword();
    expect(document.body.textContent).not.toContain("Remove password");
    const input = document.querySelector("#share-password") as HTMLInputElement;
    input.value = "s3cret";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    await click(buttonByText("Save"));
    expect(calls("set_tunnel_password")).toEqual([{ site: "app", password: "s3cret" }]);
  });

  it("forgets a typed password when the modal is cancelled", async () => {
    stubIpc({ tunnels: [quick("running")] });
    const w = await mountView();
    await openPassword();
    const input = document.querySelector("#share-password") as HTMLInputElement;
    input.value = "s3cret";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    await click(buttonByText("Cancel"));

    const vm = w.vm as unknown as { passwordInput: string; passwordSite: string | null };
    expect(vm.passwordInput).toBe("");
    expect(vm.passwordSite).toBeNull();
    expect(calls("set_tunnel_password")).toEqual([]);
  });

  it("removes the share password of a protected domain share", async () => {
    stubIpc({ tunnels: [named("running")], named: [{ site: "app", hostname: "app.example.com", protected: true }] });
    await mountView();
    expect(document.querySelector('[aria-label="Password protected"]')).not.toBeNull();
    await openPassword();
    await click(buttonByText("Remove password"));
    expect(calls("set_tunnel_password")).toEqual([{ site: "app", password: null }]);
  });
});

describe("IntegrationsView setup", () => {
  it("updates cloudflared in a log that keeps polling after it closes", async () => {
    vi.useFakeTimers();
    stubIpc({ cloudflared: { ...MANAGED, update_available: "2026.7.0" }, jobStates: ["running", "succeeded"] });
    await mountView();
    await openSetup();
    await click(buttonByText("Update to 2026.7.0"));

    expect(dialogTitles()).not.toContain("Share setup");
    expect(document.querySelector('[data-testid="tool-install-title"]')?.textContent).toBe("Updating cloudflared");
    expect(useOperations().isRunning("cloudflared-install")).toBe(true);
    await click(buttonByText("Close"));
    expect(document.querySelector('[data-testid="tool-install-title"]')).toBeNull();

    await vi.advanceTimersByTimeAsync(600);
    await flushPromises();
    expect(calls("job_status")).toHaveLength(2);
    expect(useOperations().isRunning("cloudflared-install")).toBe(false);
    expect(toastTitles()).toContain("Updated cloudflared 2026.6.1");
  });

  it("closes Setup before the login dialog opens", async () => {
    stubIpc({ cloudflared: { ...MANAGED, logged_in: false } });
    await mountView();
    await openSetup();
    await click(buttonByText("Connect"));
    expect(calls("cloudflared_login")).toHaveLength(1);
    expect(dialogTitles()).toEqual(["Connect Cloudflare account"]);
  });

  it("starts a stopped tunnel from Setup", async () => {
    stubIpc({ named: [{ site: "app", hostname: "app.example.com" }] });
    await mountView();
    await openSetup();
    await click(document.querySelector('[data-testid="setup-tunnel"] [data-action="start"]'));
    expect(calls("start_named_tunnel")).toHaveLength(1);
  });

  it("removes the tunnel after the confirm", async () => {
    stubIpc({ tunnels: [named("running")], named: [{ site: "app", hostname: "app.example.com" }] });
    await mountView();
    await openSetup();
    await click(document.querySelector('[data-testid="setup-tunnel"] [data-action="remove"]'));
    expect(dialogTitles()).toEqual(["Remove tunnel dev?"]);
    await click(buttonByText("Remove tunnel"));
    expect(calls("delete_named_tunnel")).toEqual([{ name: "dev" }]);
  });
});

describe("IntegrationsView poll", () => {
  it("revalidates only while a tunnel exists", async () => {
    vi.useFakeTimers();
    stubIpc();
    await mountView();
    const before = calls("tunnel_status").length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(calls("tunnel_status")).toHaveLength(before);
  });

  it("fetches once when revisited with a cached tunnel", async () => {
    vi.useFakeTimers();
    stubIpc({ tunnels: [quick("running")] });
    const first = await mountView();
    first.unmount();
    const before = calls("tunnel_status").length;
    await mountView();
    expect(calls("tunnel_status")).toHaveLength(before + 1);
  });

  it("refetches the tunnel status on the 5s tick", async () => {
    vi.useFakeTimers();
    stubIpc({ tunnels: [quick("running")] });
    await mountView();
    const before = calls("tunnel_status").length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(calls("tunnel_status").length).toBeGreaterThan(before);
  });
});
