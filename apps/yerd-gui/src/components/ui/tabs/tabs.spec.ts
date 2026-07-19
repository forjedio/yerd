import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "./index";

/** A two-tab harness mirroring how PhpView drives the strip. */
function harness(unmountOnHide: boolean) {
  return defineComponent({
    setup() {
      const active = ref("a");
      return () =>
        h(
          Tabs,
          {
            modelValue: active.value,
            unmountOnHide,
            "onUpdate:modelValue": (v: string | number) => {
              active.value = String(v);
            },
          },
          () => [
            h(TabsList, null, () => [
              h(TabsTrigger, { value: "a" }, () => "Tab A"),
              h(TabsTrigger, { value: "b" }, () => "Tab B"),
            ]),
            h(TabsContent, { value: "a" }, () => h("input", { id: "in-a" })),
            h(TabsContent, { value: "b" }, () => h("input", { id: "in-b" })),
          ],
        );
    },
  });
}

describe("ui/tabs", () => {
  it("forwards v-model so activating a trigger changes the panel", async () => {
    const w = mount(harness(true));
    expect(w.findAll('[role="tab"]')[0].attributes("aria-selected")).toBe("true");

    await w.findAll('[role="tab"]')[1].trigger("mousedown");
    expect(w.findAll('[role="tab"]')[1].attributes("aria-selected")).toBe("true");
  });

  // The whole per-version design rests on this: panels stay mounted so their
  // form state survives a tab switch, but only the active one is visible.
  it("keeps inactive panels mounted but hidden when unmountOnHide is false", () => {
    const w = mount(harness(false));
    expect(w.find("#in-a").exists()).toBe(true);
    expect(w.find("#in-b").exists()).toBe(true);

    const panels = w.findAll('[role="tabpanel"]');
    expect(panels[0].attributes("hidden")).toBeUndefined();
    expect(panels[1].attributes("hidden")).toBeDefined();
  });

  it("unmounts inactive panels by default", () => {
    const w = mount(harness(true));
    expect(w.find("#in-a").exists()).toBe(true);
    expect(w.find("#in-b").exists()).toBe(false);
  });

  // Both the trigger's aria-controls and the list's tab stop settle a tick after
  // mount, once reka's item collection has registered.
  it("wires each trigger to its panel", async () => {
    const w = mount(harness(false));
    await nextTick();

    const trigger = w.findAll('[role="tab"]')[0];
    const panel = w.findAll('[role="tabpanel"]')[0];
    expect(trigger.attributes("aria-controls")).toBe(panel.attributes("id"));
    expect(panel.attributes("aria-labelledby")).toBe(trigger.attributes("id"));
  });

  it("leaves the strip reachable by keyboard", async () => {
    const w = mount(harness(false));
    await nextTick();

    expect(w.find('[role="tablist"]').attributes("tabindex")).toBe("0");
  });
});
