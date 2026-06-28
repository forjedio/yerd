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
});
