import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import Input from "./Input.vue";

describe("Input", () => {
  it("exposes focus() that moves focus to the inner field (for ⌘F)", () => {
    const wrapper = mount(Input, { attachTo: document.body });
    (wrapper.vm as unknown as { focus: () => void }).focus();
    expect(document.activeElement).toBe(wrapper.find("input").element);
    wrapper.unmount();
  });

  it("lets a caller's classes replace the conflicting defaults", () => {
    const wrapper = mount(Input, {
      props: { class: "h-[34px] pl-8 disabled:opacity-100" },
    });
    const classes = wrapper.find("input").classes();
    expect(classes).toEqual(expect.arrayContaining(["h-[34px]", "pl-8", "disabled:opacity-100"]));
    expect(classes).not.toContain("h-9");
    expect(classes).not.toContain("disabled:opacity-50");
  });
});
