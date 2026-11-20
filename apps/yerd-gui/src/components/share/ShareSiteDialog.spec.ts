import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import ShareSiteDialog from "./ShareSiteDialog.vue";
import type { Site } from "@/ipc/types";

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

function mountDialog(props: Partial<InstanceType<typeof ShareSiteDialog>["$props"]> = {}) {
  const wrapper = mount(ShareSiteDialog, {
    attachTo: document.body,
    props: {
      open: true,
      sites: [site("app"), site("blog"), site("legacy", false)],
      quickSites: ["app"],
      domainSites: ["blog"],
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
    expect(w.emitted("share")).toEqual([[{ site: "blog", kind: "quick", hostname: null }]]);
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
    expect(w.emitted("share")).toEqual([[{ site: "legacy", kind: "domain", hostname: "shop.forjed.io" }]]);
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
    expect(document.querySelector('[data-testid="share-publishes"]')?.textContent).toContain("legacy.test over HTTP.");
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
});
