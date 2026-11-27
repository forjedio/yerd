import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NgrokPane from "./NgrokPane.vue";
import type { TunnelProviderStatus } from "@/ipc/types";

const TOKEN = "2fXkAbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQa";

function status(extra: Partial<TunnelProviderStatus> = {}): TunnelProviderStatus {
  return {
    provider: "ngrok",
    readiness: "ready",
    version: "3.18.4",
    source: "system",
    path: "/opt/homebrew/bin/ngrok",
    configured: true,
    authtoken_hint: "2fXk••••••••Qa",
    ...extra,
  };
}

async function mountPane(s: TunnelProviderStatus = status(), busy: string | null = null) {
  const wrapper = mount(NgrokPane, { props: { status: s, busy, connected: true } });
  await flushPromises();
  return wrapper;
}

function button(wrapper: ReturnType<typeof mount>, text: string) {
  return wrapper.findAll("button").find((b) => b.text().trim() === text);
}

describe("NgrokPane", () => {
  it("shows the masked hint and never the token", async () => {
    const w = await mountPane();
    expect(w.get('[data-testid="token-hint"]').text()).toBe("2fXk••••••••Qa");
    expect(w.html()).not.toContain(TOKEN);
    expect(w.get('[data-testid="binary-source"]').text()).toBe("Found on PATH");
    expect(w.get('[data-testid="binary-path"]').text()).toBe("/opt/homebrew/bin/ngrok");
  });

  it("replaces the token with the trimmed value", async () => {
    const w = await mountPane();
    await button(w, "Replace")?.trigger("click");
    await w.get('input[aria-label="ngrok authtoken"]').setValue(`  ${TOKEN} `);
    expect(button(w, "Cancel")).toBeDefined();
    await button(w, "Save")?.trigger("click");
    expect(w.emitted("save-token")).toEqual([[TOKEN]]);
  });

  it("clears the token input once the saved hint changes", async () => {
    const w = await mountPane(status({ authtoken_hint: undefined, configured: false, readiness: "needs_setup" }));
    await w.get('input[aria-label="ngrok authtoken"]').setValue("abc");
    await w.setProps({ status: status() });
    expect(w.find('input[aria-label="ngrok authtoken"]').exists()).toBe(false);
    expect(w.find('[data-testid="token-hint"]').exists()).toBe(true);
  });

  it("saves the domain as typed and sends null when emptied", async () => {
    const w = await mountPane(status({ domain: "demo.ngrok-free.app" }));
    const domainSave = () => w.get('[data-testid="ngrok-domain"]').findAll("button").find((b) => b.text() === "Save");
    expect(domainSave()).toBeUndefined();
    await w.get('input[aria-label="ngrok static domain"]').setValue("https://Other.ngrok-free.app/");
    await domainSave()?.trigger("click");
    await w.get('input[aria-label="ngrok static domain"]').setValue("  ");
    await domainSave()?.trigger("click");
    expect(w.emitted("save-domain")).toEqual([["https://Other.ngrok-free.app/"], [null]]);
  });

  it("saves the domain on Enter once it changed", async () => {
    const w = await mountPane(status({ domain: "demo.ngrok-free.app" }));
    const input = w.get('input[aria-label="ngrok static domain"]');
    await input.trigger("keydown", { key: "Enter" });
    await input.setValue("other.ngrok-free.app");
    await input.trigger("keydown", { key: "Enter" });
    expect(w.emitted("save-domain")).toEqual([["other.ngrok-free.app"]]);
  });

  it("renders slotted actions in the bottom row", async () => {
    const w = mount(NgrokPane, {
      props: { status: status(), busy: null, connected: true },
      slots: { default: "<button>Done</button>" },
    });
    expect(w.findAll("button").some((b) => b.text() === "Done")).toBe(true);
  });

  it("removes the token", async () => {
    const w = await mountPane();
    await w.get('[data-action="remove-token"]').trigger("click");
    expect(w.emitted("remove-token")).toHaveLength(1);
  });

  it("points to Tooling when not installed but still saves a token", async () => {
    const w = await mountPane({ provider: "ngrok", readiness: "not_installed", configured: false });
    await w.get('[data-action="tooling"]').trigger("click");
    expect(w.emitted("tooling")).toHaveLength(1);
    expect(w.find('[data-action="remove-token"]').exists()).toBe(false);
    await w.get('input[aria-label="ngrok authtoken"]').setValue(TOKEN);
    const save = w.get('[data-testid="ngrok-token"]').findAll("button").find((b) => b.text() === "Save");
    expect(save?.attributes("disabled")).toBeUndefined();
    await save?.trigger("click");
    expect(w.emitted("save-token")).toEqual([[TOKEN]]);
  });

  it("re-checks and opens the dashboard", async () => {
    const w = await mountPane();
    await w.get('[data-action="recheck"]').trigger("click");
    await w.findAll("button").find((b) => b.text().includes("dashboard.ngrok.com"))?.trigger("click");
    expect(w.emitted("recheck")).toHaveLength(1);
    expect(w.emitted("open-url")).toEqual([["https://dashboard.ngrok.com"]]);
  });
});
