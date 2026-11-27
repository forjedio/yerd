import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import EditUpstreamDialog from "./EditUpstreamDialog.vue";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountDialog(props: Record<string, unknown> = {}) {
  const w = mount(EditUpstreamDialog, {
    attachTo: document.body,
    props: { open: true, host: "grafana.test", current: "http://localhost:9011", ...props },
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

function save(w: VueWrapper) {
  const btn = w.findAll("button").find((b) => b.text().trim() === "Save");
  if (!btn) throw new Error('no button "Save"');
  return btn;
}

describe("EditUpstreamDialog", () => {
  it("pre-fills the current upstream without its scheme and shows the hint", async () => {
    const w = await mountDialog();
    expect((w.get("#edit-upstream-url").element as HTMLInputElement).value).toBe("localhost:9011");
    expect(w.find('[data-testid="scheme-hint"]').exists()).toBe(true);
    expect(w.get('[data-testid="edit-upstream-host"]').text()).toBe("grafana.test");
  });

  it("disables Save when the field is empty", async () => {
    const w = await mountDialog();
    await w.get("#edit-upstream-url").setValue("   ");
    expect(save(w).attributes("disabled")).toBeDefined();
  });

  it("disables Save until the normalised input differs from the current upstream", async () => {
    const w = await mountDialog();
    expect(save(w).attributes("disabled")).toBeDefined();
    await w.get("#edit-upstream-url").setValue("http://localhost:9011/");
    expect(save(w).attributes("disabled")).toBeDefined();
    await w.get("#edit-upstream-url").trigger("keydown", { key: "Enter" });
    expect(w.emitted("save")).toBeUndefined();
    await w.get("#edit-upstream-url").setValue("localhost:9012");
    expect(save(w).attributes("disabled")).toBeUndefined();
  });

  it.each([
    ["3001", "http://127.0.0.1:3001"],
    ["https://up:8443/", "https://up:8443"],
  ])("normalises %s to %s on save and closes", async (typed, expected) => {
    const w = await mountDialog();
    await w.get("#edit-upstream-url").setValue(typed);
    await save(w).trigger("click");
    expect(w.emitted("save")).toEqual([[expected]]);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("hides the scheme hint once a scheme is typed", async () => {
    const w = await mountDialog();
    await w.get("#edit-upstream-url").setValue("https://up:8443");
    expect(w.find('[data-testid="scheme-hint"]').exists()).toBe(false);
  });

  it("submits on Enter", async () => {
    const w = await mountDialog();
    await w.get("#edit-upstream-url").setValue("localhost:3001");
    await w.get("#edit-upstream-url").trigger("keydown", { key: "Enter" });
    expect(w.emitted("save")).toEqual([["http://localhost:3001"]]);
  });
});
