import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import NewProxyDialog from "./NewProxyDialog.vue";
import type { SiteEntry } from "@/ipc/types";

function site(name: string, secure = false): SiteEntry {
  return { name, document_root: `/srv/${name}`, php: "8.4", secure, kind: "linked" };
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountDialog(props: Record<string, unknown> = {}) {
  const w = mount(NewProxyDialog, {
    attachTo: document.body,
    props: { open: true, tld: "test", sites: [site("zeta"), site("codestash", true)], trusted: true, ...props },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

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

function mode(w: VueWrapper, value: "domain" | "path") {
  return w.get(`[data-mode="${value}"]`);
}

async function pathMode(w: VueWrapper) {
  await mode(w, "path").trigger("click");
  await flushPromises();
}

describe("NewProxyDialog modes", () => {
  it("opens on a whole domain by default, or on the initial mode", async () => {
    expect(mode(await mountDialog(), "domain").attributes("data-state")).toBe("checked");
    const path = await mountDialog({ initialMode: "path" });
    expect(mode(path, "path").attributes("data-state")).toBe("checked");
    expect(path.find("#proxy-prefix").exists()).toBe(true);
  });

  it("disables the path mode without sites", async () => {
    const w = await mountDialog({ sites: [], initialMode: "path" });
    expect(mode(w, "path").attributes("disabled")).toBeDefined();
    expect(mode(w, "path").attributes("title")).toBe("Add a site first");
    expect(mode(w, "domain").attributes("data-state")).toBe("checked");
  });

  it.each([
    [true, "Certificate from Yerd local CA — trusted"],
    [false, "Certificate from Yerd local CA — not trusted, browsers will warn"],
    [null, "Certificate from Yerd local CA — not trusted, browsers will warn"],
  ] as const)("words HTTPS for trust %s", async (trusted, text) => {
    const w = await mountDialog({ trusted });
    expect(w.get('[data-testid="dialog-https-subtitle"]').text()).toBe(text);
  });
});

describe("NewProxyDialog whole domain", () => {
  it("validates the name, strips a typed .test and emits a normalised request", async () => {
    const w = await mountDialog();
    const add = () => button(w, "Add proxy");
    await w.get("#proxy-url").setValue("localhost:3000/");
    expect(add().attributes("disabled")).toBeDefined();
    await w.get("#proxy-name").setValue("api..account");
    expect(add().attributes("disabled")).toBeDefined();
    await w.get("#proxy-name").setValue("Grafana.test");
    expect(w.get('[data-testid="add-summary"]').text()).toBe("Grafana.testlocalhost:3000");
    await w.get('button[aria-label="HTTPS"]').trigger("click");
    await add().trigger("click");
    expect(w.emitted("update:open")).toEqual([[false]]);
    expect(w.emitted("addProxy")).toEqual([[{ name: "Grafana", url: "http://localhost:3000", secure: true }]]);
  });

  it("hides the http:// hint once a scheme is typed", async () => {
    const w = await mountDialog();
    expect(w.find('[data-testid="scheme-hint"]').exists()).toBe(true);
    await w.get("#proxy-url").setValue("https://localhost:8443");
    expect(w.find('[data-testid="scheme-hint"]').exists()).toBe(false);
  });

  it("submits on Return in a field", async () => {
    const w = await mountDialog();
    await w.get("#proxy-name").setValue("grafana");
    await w.get("#proxy-url").setValue("localhost:3000");
    await w.get("#proxy-url").trigger("keydown", { key: "Enter" });
    expect(w.emitted("addProxy")).toEqual([[{ name: "grafana", url: "http://localhost:3000", secure: false }]]);
  });

  it("resets its fields on reopen", async () => {
    const w = await mountDialog();
    await w.get("#proxy-name").setValue("grafana");
    await pathMode(w);
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect(mode(w, "domain").attributes("data-state")).toBe("checked");
    expect((w.get("#proxy-name").element as HTMLInputElement).value).toBe("");
  });
});

describe("NewProxyDialog path on a site", () => {
  it("defaults to the first site by name and needs a / prefix", async () => {
    const w = await mountDialog();
    await pathMode(w);
    expect((w.get("#proxy-site").element as HTMLSelectElement).value).toBe("codestash");
    await w.get("#proxy-url").setValue("127.0.0.1:8081");
    await w.get("#proxy-prefix").setValue("live");
    expect(button(w, "Add path").attributes("disabled")).toBeDefined();
    await w.get("#proxy-prefix").setValue("/live");
    expect(w.get('[data-testid="add-summary"]').text()).toBe("codestash.test/live127.0.0.1:8081");
    await button(w, "Add path").trigger("click");
    expect(w.emitted("addPath")).toEqual([[{ site: "codestash", prefix: "/live", url: "http://127.0.0.1:8081" }]]);
  });

  it("explains wss:// from the site's HTTPS", async () => {
    const w = await mountDialog();
    await pathMode(w);
    await w.get("#proxy-prefix").setValue("/live");
    expect(w.get('[data-testid="wss-row"]').text()).toBe(
      "codestash.test is on HTTPS, so wss://codestash.test/live works too",
    );
    await w.get("#proxy-site").setValue("zeta");
    expect(w.get('[data-testid="wss-row"]').text()).toBe(
      "zeta.test is on HTTP. Turn on HTTPS for the site to use wss://",
    );
  });
});
