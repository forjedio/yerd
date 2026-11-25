import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import ShareSiteDialog from "./ShareSiteDialog.vue";
import type { Site, TunnelProviderStatus } from "@/ipc/types";
import type { ShareItem } from "@/lib/share";

const Pass = { template: "<div><slot /></div>" };
const ComboboxStub = {
  name: "Combobox",
  props: ["modelValue", "options", "disabled"],
  emits: ["update:modelValue"],
  template:
    '<div data-testid="site-combobox"><button v-for="o in options" :key="o.value" type="button" :data-option="o.value" @click="$emit(\'update:modelValue\', o.value)">{{ o.label }}</button></div>',
};

const mounted: VueWrapper[] = [];

function site(name: string, secure = true): Site {
  return { name, document_root: `/srv/${name}`, php: "8.4", secure } as Site;
}

function share(overrides: Partial<ShareItem>): ShareItem {
  return {
    key: "cloudflare:quick:app",
    provider: "cloudflare",
    kind: "quick",
    site: "app",
    staticDomain: false,
    url: null,
    host: null,
    state: "running",
    protected: false,
    lastError: null,
    warnings: [],
    ...overrides,
  };
}

const ITEMS: ShareItem[] = [share({}), share({ key: "cloudflare:domain:blog", kind: "domain", site: "blog" })];

const PROVIDERS: TunnelProviderStatus[] = [
  { provider: "cloudflare", readiness: "ready", configured: true },
  { provider: "ngrok", readiness: "ready", configured: true, domain: "demo.ngrok-free.app" },
  { provider: "zrok", readiness: "needs_setup", configured: false },
];

function mountDialog(props: Partial<InstanceType<typeof ShareSiteDialog>["$props"]> = {}) {
  const wrapper = mount(ShareSiteDialog, {
    attachTo: document.body,
    props: {
      open: true,
      sites: [site("app"), site("blog"), site("legacy", false)],
      items: ITEMS,
      providers: PROVIDERS,
      defaultProvider: "cloudflare",
      zone: "forjed.io",
      domainReady: true,
      tld: "test",
      busy: false,
      locked: false,
      ...props,
    },
    global: { stubs: { DialogPortal: Pass, Combobox: ComboboxStub } },
  });
  mounted.push(wrapper);
  return wrapper;
}

function options(): string[] {
  return Array.from(document.querySelectorAll("[data-option]")).map((el) => el.getAttribute("data-option") ?? "");
}

async function pickSite(name: string) {
  (document.querySelector(`[data-option="${name}"]`) as HTMLElement).click();
  await flushPromises();
}

async function pickKind(kind: "quick" | "domain") {
  (document.querySelector(`[data-kind="${kind}"]`) as HTMLElement).click();
  await flushPromises();
}

async function pickProvider(provider: string) {
  (document.querySelector(`[data-provider="${provider}"]`) as HTMLElement).click();
  await flushPromises();
}

async function pickUrl(choice: "static" | "random") {
  (document.querySelector(`[data-url="${choice}"]`) as HTMLElement).click();
  await flushPromises();
}

function submit(): HTMLButtonElement {
  return document.querySelector('[data-testid="share-submit"]') as HTMLButtonElement;
}

function hostInput(): HTMLInputElement | null {
  return document.querySelector("#share-hostname");
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("ShareSiteDialog", () => {
  it("shares a quick link for the chosen site", async () => {
    const w = mountDialog();
    await flushPromises();
    expect(options()).toEqual(["blog", "legacy"]);
    expect(submit().disabled).toBe(true);
    await pickSite("blog");
    submit().click();
    expect(w.emitted("share")).toEqual([
      [{ site: "blog", provider: "cloudflare", kind: "quick", hostname: null, randomUrl: false }],
    ]);
  });

  it("offers sites not on the domain and prefills the hostname until it is edited", async () => {
    const w = mountDialog();
    await flushPromises();
    await pickKind("domain");
    expect(options()).toEqual(["app", "legacy"]);
    await pickSite("app");
    expect(hostInput()?.value).toBe("app.forjed.io");
    hostInput()!.value = "shop.forjed.io";
    hostInput()!.dispatchEvent(new Event("input"));
    await pickSite("legacy");
    expect(hostInput()?.value).toBe("shop.forjed.io");
    submit().click();
    expect(w.emitted("share")).toEqual([
      [{ site: "legacy", provider: "cloudflare", kind: "domain", hostname: "shop.forjed.io", randomUrl: false }],
    ]);
  });

  it("prefills nothing and blocks Share until the zone resolves", async () => {
    const w = mountDialog({ zone: null });
    await flushPromises();
    await pickKind("domain");
    await pickSite("app");
    expect(hostInput()?.value).toBe("");
    expect(hostInput()?.placeholder).toBe("site.example.com");
    expect(submit().disabled).toBe(true);
    expect(document.querySelector('[data-testid="host-hint"]')?.textContent).toContain("couldn't read your Cloudflare domain");

    hostInput()!.value = "app.example.com";
    hostInput()!.dispatchEvent(new Event("input"));
    await flushPromises();
    expect(submit().disabled).toBe(true);

    await w.setProps({ zone: "forjed.io" });
    await flushPromises();
    expect(submit().disabled).toBe(true);
    expect(document.querySelector('[data-testid="host-hint"]')?.textContent).toContain("Use a hostname on forjed.io");
  });

  it("re-suggests an unedited hostname once the zone resolves", async () => {
    const w = mountDialog({ zone: null });
    await flushPromises();
    await pickKind("domain");
    await pickSite("app");
    await w.setProps({ zone: "forjed.io" });
    await flushPromises();
    expect(hostInput()?.value).toBe("app.forjed.io");
    expect(submit().disabled).toBe(false);
    expect(document.querySelector('[data-testid="host-hint"]')).toBeNull();
  });

  it("disables Share without a hostname", async () => {
    mountDialog();
    await flushPromises();
    await pickKind("domain");
    await pickSite("app");
    hostInput()!.value = "  ";
    hostInput()!.dispatchEvent(new Event("input"));
    await flushPromises();
    expect(submit().disabled).toBe(true);
  });

  it("disables Your domain until an account and tunnel exist", async () => {
    mountDialog({ domainReady: false });
    await flushPromises();
    const domain = document.querySelector('[data-kind="domain"]') as HTMLButtonElement;
    expect(domain.disabled).toBe(true);
    expect(document.querySelector('[data-testid="domain-detail"]')?.textContent).toContain("Needs a Cloudflare account");
  });

  it("reads HTTP for an insecure site", async () => {
    mountDialog();
    await flushPromises();
    await pickSite("legacy");
    expect(document.querySelector('[data-testid="share-publishes"]')?.textContent).toContain(
      "legacy.test over HTTP through Cloudflare.",
    );
  });

  it("disables Share while another action holds the lock", async () => {
    mountDialog({ locked: true });
    await flushPromises();
    await pickSite("blog");
    expect(submit().disabled).toBe(true);
  });

  it("stays open and locked while its share is in flight", async () => {
    const w = mountDialog({ busy: true, locked: true });
    await flushPromises();
    (document.querySelector('[aria-label="Close"]') as HTMLElement).click();
    await flushPromises();
    const cancel = Array.from(document.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Cancel");
    expect(cancel?.disabled).toBe(true);
    expect(w.emitted("update:open")).toBeUndefined();
  });

  it("preselects the default provider, or the one it was opened for", async () => {
    mountDialog({ defaultProvider: "ngrok" });
    await flushPromises();
    expect(document.querySelector('[data-provider="ngrok"]')?.getAttribute("aria-checked")).toBe("true");
    mounted.forEach((m) => m.unmount());
    document.body.innerHTML = "";
    mountDialog({ defaultProvider: "ngrok", initialProvider: "zrok", initialSite: "app" });
    await flushPromises();
    expect(document.querySelector('[data-provider="zrok"]')?.getAttribute("aria-checked")).toBe("true");
  });

  it("blocks a provider that is not ready and offers to set it up", async () => {
    const w = mountDialog();
    await flushPromises();
    await pickSite("blog");
    await pickProvider("zrok");
    expect(document.querySelector('[data-testid="provider-not-ready"]')?.textContent).toContain(
      "zrok needs setting up first",
    );
    expect(submit().disabled).toBe(true);
    (document.querySelector('[data-testid="provider-setup"]') as HTMLElement).click();
    expect(w.emitted("setup")).toEqual([["zrok"]]);
  });

  it("preselects ngrok's static domain and lets Random URL replace it", async () => {
    const w = mountDialog();
    await flushPromises();
    await pickProvider("ngrok");
    await pickSite("blog");
    expect(document.querySelector('[data-url="static"]')?.getAttribute("aria-checked")).toBe("true");
    submit().click();
    await pickUrl("random");
    submit().click();
    expect(w.emitted("share")).toEqual([
      [{ site: "blog", provider: "ngrok", kind: "quick", hostname: null, randomUrl: false }],
      [{ site: "blog", provider: "ngrok", kind: "quick", hostname: null, randomUrl: true }],
    ]);
  });

  it("disables the static domain while another site's share holds it", async () => {
    const holder = share({ key: "ngrok:quick:legacy", provider: "ngrok", site: "legacy", url: "https://demo.ngrok-free.app" });
    mountDialog({ items: [...ITEMS, holder] });
    await flushPromises();
    await pickProvider("ngrok");
    const staticBtn = document.querySelector('[data-url="static"]') as HTMLButtonElement;
    expect(staticBtn.disabled).toBe(true);
    expect(document.querySelector('[data-testid="static-in-use"]')?.textContent).toContain("In use by legacy's share");
    expect(document.querySelector('[data-url="random"]')?.getAttribute("aria-checked")).toBe("true");
  });

  it("notes when the site is already shared through another provider", async () => {
    mountDialog();
    await flushPromises();
    await pickProvider("ngrok");
    await pickSite("app");
    expect(document.querySelector('[data-testid="shared-elsewhere"]')?.textContent).toContain(
      "app is already shared through Cloudflare.",
    );
  });
});
