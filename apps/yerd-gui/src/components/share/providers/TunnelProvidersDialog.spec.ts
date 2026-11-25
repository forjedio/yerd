import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTunnelProviders = vi.fn();
const setNgrokAuthtoken = vi.fn();
const setNgrokDomain = vi.fn();
const enableZrok = vi.fn();
const disableZrok = vi.fn();
const setDefaultTunnelProvider = vi.fn();
vi.mock("@/ipc/client", () => ({
  listTunnelProviders: (...a: unknown[]) => listTunnelProviders(...a),
  setNgrokAuthtoken: (...a: unknown[]) => setNgrokAuthtoken(...a),
  setNgrokDomain: (...a: unknown[]) => setNgrokDomain(...a),
  enableZrok: (...a: unknown[]) => enableZrok(...a),
  disableZrok: (...a: unknown[]) => disableZrok(...a),
  setDefaultTunnelProvider: (...a: unknown[]) => setDefaultTunnelProvider(...a),
  IpcError: class IpcError extends Error {},
}));

import TunnelProvidersDialog from "./TunnelProvidersDialog.vue";
import { useToast } from "@/composables/useToast";
import type { CloudflaredStatus, NamedTunnelMeta, TunnelProvider, TunnelProvidersResponse } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };
const TOKEN = "zrokAccountToken123";
const mounted: VueWrapper[] = [];
const cloudflared: CloudflaredStatus = { installed: true, version: "2026.6.1", source: "managed", logged_in: true };

function reply(): TunnelProvidersResponse {
  return {
    type: "tunnel_providers",
    providers: [
      { provider: "cloudflare", readiness: "ready", configured: true, version: "2026.6.1", source: "managed" },
      { provider: "ngrok", readiness: "needs_setup", configured: false, version: "3.18.4", source: "managed" },
      { provider: "zrok", readiness: "not_installed", configured: false },
    ],
    default_provider: "cloudflare",
  };
}

async function mountDialog(
  extra: { cloudflared?: CloudflaredStatus; tunnel?: NamedTunnelMeta; initial?: TunnelProvider } = {},
) {
  const wrapper = mount(TunnelProvidersDialog, {
    attachTo: document.body,
    props: {
      open: true,
      cloudflared,
      zone: "forjed.io",
      tunnel: null,
      entry: null,
      exposedCount: 0,
      busy: null,
      connected: true,
      ...extra,
    },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function el<T extends HTMLElement = HTMLElement>(selector: string): T {
  return document.querySelector(selector) as T;
}

function buttonWith(text: string): HTMLButtonElement | undefined {
  return Array.from(document.querySelectorAll("button")).find((b) => b.textContent?.trim() === text);
}

async function select(provider: string): Promise<void> {
  el(`[data-testid="provider-${provider}"]`).click();
  await flushPromises();
}

async function type(selector: string, value: string): Promise<void> {
  const input = el<HTMLInputElement>(selector);
  input.value = value;
  input.dispatchEvent(new Event("input"));
  await flushPromises();
}

beforeEach(() => {
  listTunnelProviders.mockReset().mockResolvedValue(reply());
  for (const f of [setNgrokAuthtoken, setNgrokDomain, enableZrok, disableZrok, setDefaultTunnelProvider]) {
    f.mockReset().mockResolvedValue(undefined);
  }
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  useToast().toasts.value.forEach((t) => useToast().dismiss(t.id));
});

describe("TunnelProvidersDialog", () => {
  it("lists the three providers with their readiness", async () => {
    await mountDialog();
    expect(listTunnelProviders).toHaveBeenCalledTimes(1);
    expect(el('[data-testid="provider-cloudflare"]').textContent).toContain("Ready");
    expect(el('[data-testid="provider-ngrok"]').textContent).toContain("Needs setup");
    expect(el('[data-testid="provider-zrok"]').textContent).toContain("Not installed");
    expect(el('[data-testid="setup-account"]')).not.toBeNull();
  });

  it("opens on the provider it was asked for", async () => {
    await mountDialog({ initial: "zrok" });
    expect(el('[data-testid="provider-zrok"]').getAttribute("aria-pressed")).toBe("true");
    expect(el('[data-testid="setup-account"]')).toBeNull();
  });

  it("shows the ngrok pane when ngrok is selected", async () => {
    await mountDialog();
    await select("ngrok");
    expect(el('[data-testid="ngrok-token"]')).not.toBeNull();
    expect(el('[data-testid="setup-account"]')).toBeNull();
  });

  it("bubbles the Cloudflare pane and Tooling events", async () => {
    const w = await mountDialog({ cloudflared: { ...cloudflared, logged_in: false } });
    buttonWith("Connect")?.click();
    expect(w.emitted("login")).toHaveLength(1);
    await select("ngrok");
    expect(el('[data-action="tooling"]')).toBeNull();
    await select("zrok");
    el('[data-action="tooling"]').click();
    expect(w.emitted("tooling")).toHaveLength(1);
  });

  it("bubbles Remove tunnel from the Cloudflare pane", async () => {
    const w = await mountDialog({ tunnel: { name: "yerd-mac", uuid: "6f1c" } });
    el('[data-action="remove"]').click();
    expect(w.emitted("remove")).toHaveLength(1);
  });

  it("saves and removes the ngrok token, then refetches", async () => {
    listTunnelProviders.mockResolvedValue({
      ...reply(),
      providers: reply().providers.map((p) =>
        p.provider === "ngrok" ? { ...p, readiness: "ready", configured: true, authtoken_hint: "abcd••••••••yz" } : p,
      ),
    });
    await mountDialog();
    await select("ngrok");
    el('[data-action="remove-token"]').click();
    await flushPromises();
    expect(setNgrokAuthtoken).toHaveBeenCalledWith(null);
    buttonWith("Replace")?.click();
    await flushPromises();
    await type('input[aria-label="ngrok authtoken"]', "tok_new");
    Array.from(el('[data-testid="ngrok-token"]').querySelectorAll("button"))
      .find((b) => b.textContent?.trim() === "Save")
      ?.click();
    await flushPromises();
    expect(setNgrokAuthtoken).toHaveBeenLastCalledWith("tok_new");
    expect(listTunnelProviders).toHaveBeenCalledTimes(3);
  });

  it("enables zrok and keeps the token out of an error toast", async () => {
    listTunnelProviders.mockResolvedValue({
      ...reply(),
      providers: reply().providers.map((p) =>
        p.provider === "zrok" ? { ...p, readiness: "needs_setup", version: "2.0.11", source: "managed" } : p,
      ),
    });
    enableZrok.mockRejectedValue(new Error("zrok enable failed: unable to enable with token ***"));
    await mountDialog();
    await select("zrok");
    await type('input[aria-label="zrok account token"]', TOKEN);
    el('[data-action="enable"]').click();
    await flushPromises();
    expect(enableZrok).toHaveBeenCalledWith(TOKEN);
    const toasts = JSON.stringify(useToast().toasts.value);
    expect(toasts).toContain("Couldn't enable zrok");
    expect(toasts).not.toContain(TOKEN);
  });

  it("sets the default provider", async () => {
    await mountDialog();
    const sel = el<HTMLSelectElement>('select[aria-label="Default for new shares"]');
    sel.value = "ngrok";
    sel.dispatchEvent(new Event("change"));
    await flushPromises();
    expect(setDefaultTunnelProvider).toHaveBeenCalledWith("ngrok");
    expect(listTunnelProviders).toHaveBeenCalledTimes(2);
  });
});
