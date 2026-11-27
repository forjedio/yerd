import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import MailSetupDialog from "./MailSetupDialog.vue";
import type { MailStatus } from "@/ipc/types";

const CAPTURE: MailStatus = { enabled: true, port: 2526, listening: true, count: 0 };
const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountDialog(props: Record<string, unknown> = {}) {
  const wrapper = mount(MailSetupDialog, {
    attachTo: document.body,
    props: {
      open: true,
      capture: CAPTURE,
      envText: "MAIL_PORT=2526",
      toggling: false,
      fromName: "",
      fromAddress: "",
      "onUpdate:fromAddress": (v: string) => wrapper.setProps({ fromAddress: v }),
      ...props,
    },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function button(text: string): HTMLButtonElement {
  const btn = Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

function query<T extends Element>(selector: string): T {
  const el = document.body.querySelector<T>(selector);
  if (!el) throw new Error(`no ${selector}`);
  return el;
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("MailSetupDialog", () => {
  it("shows the SMTP address and the .env block", async () => {
    await mountDialog();
    expect(query('[data-testid="smtp-address"]').textContent).toContain("127.0.0.1:2526");
    expect(query('[data-testid="env-block"]').textContent).toBe("MAIL_PORT=2526");
  });

  it("toggles capture off and locks while toggling", async () => {
    const w = await mountDialog();
    query<HTMLButtonElement>('[aria-label="Capture outgoing mail"]').click();
    expect(w.emitted("toggle")).toEqual([[false]]);
    await w.setProps({ toggling: true });
    expect(query<HTMLButtonElement>('[aria-label="Capture outgoing mail"]').disabled).toBe(true);
  });

  it("updates the From address model", async () => {
    const w = await mountDialog();
    const input = query<HTMLInputElement>("#mail-from-address");
    input.value = "shop@x.test";
    input.dispatchEvent(new Event("input"));
    await flushPromises();
    expect(w.props("fromAddress")).toBe("shop@x.test");
  });

  it("emits copy, change port and closes on Done", async () => {
    const w = await mountDialog();
    query<HTMLButtonElement>('[aria-label="Copy .env configuration"]').click();
    button("Change port").click();
    button("Done").click();
    expect(w.emitted("copy")).toHaveLength(1);
    expect(w.emitted("changePort")).toHaveLength(1);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });
});
