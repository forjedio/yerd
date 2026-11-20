import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import DumpsSetupDialog from "./DumpsSetupDialog.vue";
import type { DumpsStatusResponse } from "@/ipc/types";

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

function status(overrides: Partial<DumpsStatusResponse> = {}): DumpsStatusResponse {
  return {
    type: "dumps_status",
    enabled: true,
    port: 2304,
    running: true,
    persist: false,
    extensions: [
      { version: "8.4", present: true },
      { version: "8.3", present: false },
      { version: "7.4", present: false, legacy: true },
    ],
    counts: COUNTS,
    features: { http: false },
    ...overrides,
  };
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountDialog(s: DumpsStatusResponse) {
  const wrapper = mount(DumpsSetupDialog, {
    attachTo: document.body,
    props: { open: true, status: s, busy: null },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function chips(): string[] {
  return Array.from(document.body.querySelectorAll('[data-testid="ext-chip"]')).map((c) => c.textContent?.trim() ?? "");
}

function switchFor(label: string): HTMLButtonElement {
  const el = document.body.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
  if (!el) throw new Error(`no switch ${label}`);
  return el;
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("DumpsSetupDialog", () => {
  it("labels each PHP version's extension, legacy as too old", async () => {
    await mountDialog(status());
    expect(chips()).toEqual(["INSTALLED", "NOT INSTALLED", "TOO OLD — NO DUMPS"]);
    expect(document.body.querySelector('[data-testid="dump-server-address"]')?.textContent).toContain("127.0.0.1:2304");
  });

  it("emits switch changes and disables the signals while interception is off", async () => {
    const on = await mountDialog(status());
    switchFor("Outgoing HTTP (off by default)").click();
    switchFor("Intercept dumps").click();
    expect(on.emitted("setFeature")?.[0]).toEqual(["http", true]);
    expect(on.emitted("setEnabled")?.[0]).toEqual([false]);
    on.unmount();
    mounted.length = 0;
    document.body.innerHTML = "";
    await mountDialog(status({ enabled: false }));
    expect(switchFor("Cache events").disabled).toBe(true);
  });
});
