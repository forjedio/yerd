import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import Button from "./Button.vue";

describe("Button", () => {
  it("lets a caller's size classes replace the variant's height and width", () => {
    const wrapper = mount(Button, { props: { size: "icon", class: "size-[34px] shrink-0" } });
    const classes = wrapper.find("button").classes();
    expect(classes).toEqual(expect.arrayContaining(["size-[34px]", "shrink-0"]));
    expect(classes).not.toContain("h-9");
    expect(classes).not.toContain("w-9");
  });

  it("lets a caller's spacing and type classes replace the defaults", () => {
    const wrapper = mount(Button, {
      props: { variant: "outline", class: "h-[34px] gap-1.5 px-3 text-[13px]" },
    });
    const classes = wrapper.find("button").classes();
    expect(classes).toEqual(expect.arrayContaining(["h-[34px]", "gap-1.5", "px-3", "text-[13px]"]));
    expect(classes).not.toContain("h-9");
    expect(classes).not.toContain("gap-2");
    expect(classes).not.toContain("px-4");
    expect(classes).not.toContain("text-sm");
  });

  it("defaults to type=button and keeps the variant classes without a caller class", () => {
    const button = mount(Button).find("button");
    expect(button.attributes("type")).toBe("button");
    expect(button.classes()).toEqual(expect.arrayContaining(["h-9", "px-4", "bg-primary"]));
  });
});
