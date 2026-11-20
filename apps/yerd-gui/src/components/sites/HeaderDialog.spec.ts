import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import HeaderDialog from "./HeaderDialog.vue";

function mountDialog(editing: { site: string; name: string; value: string } | null = null) {
  return mount(HeaderDialog, { attachTo: document.body, props: { open: true, editing } });
}

function field(id: string): HTMLInputElement {
  return document.querySelector(`#${id}`) as HTMLInputElement;
}

function action(label: string): HTMLButtonElement {
  return [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === label)!;
}

async function type(id: string, value: string): Promise<void> {
  field(id).value = value;
  field(id).dispatchEvent(new Event("input"));
  await Promise.resolve();
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("HeaderDialog", () => {
  it("needs both a name and a value, and submits them trimmed", async () => {
    const w = mountDialog();
    await type("header-name", " X-Debug ");
    expect(action("Add header").disabled).toBe(true);
    await type("header-value", " 1 ");
    expect(action("Add header").disabled).toBe(false);
    action("Add header").click();
    expect(w.emitted("submit")).toEqual([["X-Debug", "1"]]);
    w.unmount();
  });

  it("prefills an edit and only saves a change", async () => {
    const w = mountDialog({ site: "api", name: "X-Frame-Options", value: "DENY" });
    expect(field("header-name").value).toBe("X-Frame-Options");
    expect(field("header-value").value).toBe("DENY");
    expect(action("Save").disabled).toBe(true);
    await type("header-value", "SAMEORIGIN");
    expect(action("Save").disabled).toBe(false);
    w.unmount();
  });
});
