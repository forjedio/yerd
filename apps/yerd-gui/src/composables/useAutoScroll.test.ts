import { mount } from "@vue/test-utils";
import { defineComponent, h, nextTick, ref } from "vue";
import { describe, expect, it } from "vitest";

import { useAutoScroll } from "./useAutoScroll";

function harness(lines: ReturnType<typeof ref<string[]>>) {
  return defineComponent({
    setup() {
      const box = useAutoScroll(() => lines.value?.length ?? 0);
      return () => h("pre", { ref: box }, (lines.value ?? []).join("\n"));
    },
  });
}

describe("useAutoScroll", () => {
  it("scrolls the box to its end when the line count changes", async () => {
    const lines = ref<string[]>(["one"]);
    const wrapper = mount(harness(lines));
    const el = wrapper.element as HTMLElement;
    Object.defineProperty(el, "scrollHeight", { configurable: true, value: 480 });
    el.scrollTop = 0;

    lines.value = ["one", "two"];
    await nextTick();
    expect(el.scrollTop).toBe(480);
  });

  it("leaves the box alone while the count is unchanged", async () => {
    const lines = ref<string[]>(["one"]);
    const wrapper = mount(harness(lines));
    const el = wrapper.element as HTMLElement;
    Object.defineProperty(el, "scrollHeight", { configurable: true, value: 480 });
    el.scrollTop = 7;

    lines.value = ["uno"];
    await nextTick();
    expect(el.scrollTop).toBe(7);
  });
});
