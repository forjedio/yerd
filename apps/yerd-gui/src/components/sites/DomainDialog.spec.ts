import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import DomainDialog from "./DomainDialog.vue";

function mountDialog(props: Partial<InstanceType<typeof DomainDialog>["$props"]> = {}) {
  return mount(DomainDialog, {
    attachTo: document.body,
    props: { open: true, name: "blog", tld: "test", editing: null, ...props },
  });
}

function input(): HTMLInputElement {
  return document.querySelector("#add-domain") as HTMLInputElement;
}

function saveButton(label: string): HTMLButtonElement {
  return [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === label)!;
}

async function type(value: string): Promise<void> {
  const el = input();
  el.value = value;
  el.dispatchEvent(new Event("input"));
  await Promise.resolve();
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("DomainDialog", () => {
  it("gates Add on the domain shape and submits the trimmed value", async () => {
    const w = mountDialog();
    await type("nope");
    expect(saveButton("Add domain").disabled).toBe(true);
    await type("  api.blog.test ");
    expect(saveButton("Add domain").disabled).toBe(false);
    saveButton("Add domain").click();
    expect(w.emitted("submit")).toEqual([["api.blog.test"]]);
    w.unmount();
  });

  it("accepts a wildcard", async () => {
    const w = mountDialog();
    await type("*.blog.test");
    expect(saveButton("Add domain").disabled).toBe(false);
    w.unmount();
  });

  it("warns about a wrong TLD without blocking", async () => {
    const w = mountDialog();
    await type("corp.dev");
    expect(document.body.textContent).toContain("Expected to end in .test");
    expect(saveButton("Add domain").disabled).toBe(false);
    w.unmount();
  });

  it("prefills an edit and only saves a change", async () => {
    const w = mountDialog({ editing: "api.blog.test" });
    expect(document.body.textContent).toContain("Edit domain");
    expect(input().value).toBe("api.blog.test");
    expect(saveButton("Save").disabled).toBe(true);
    await type("shop.blog.test");
    expect(saveButton("Save").disabled).toBe(false);
    w.unmount();
  });

  it("notes the WordPress site URL when editing its primary", () => {
    const w = mountDialog({ editing: "blog.test", editingPrimary: true, isWordpress: true });
    expect(document.body.textContent).toContain("WordPress site");
    w.unmount();
  });
});
