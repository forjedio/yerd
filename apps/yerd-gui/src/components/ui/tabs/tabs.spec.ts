import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "./index";

/**
 * A two-tab harness mirroring how PhpView drives the rail.
 *
 * Options are spread in only when supplied, never passed as `undefined`:
 * reka's `useForwardProps` keys off whether a prop is *present*, so an
 * explicit `undefined` would still shadow reka's own default and make a
 * "by default" assertion test the value we passed instead.
 */
function harness(opts: { unmountOnHide?: boolean; orientation?: "horizontal" | "vertical" } = {}) {
  return defineComponent({
    setup() {
      const active = ref("a");
      return () =>
        h(
          Tabs,
          {
            modelValue: active.value,
            ...(opts.unmountOnHide === undefined ? {} : { unmountOnHide: opts.unmountOnHide }),
            ...(opts.orientation === undefined ? {} : { orientation: opts.orientation }),
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
    const w = mount(harness());
    expect(w.findAll('[role="tab"]')[0].attributes("aria-selected")).toBe("true");

    await w.findAll('[role="tab"]')[1].trigger("mousedown");
    expect(w.findAll('[role="tab"]')[1].attributes("aria-selected")).toBe("true");
  });

  it("keeps inactive panels mounted but hidden when unmountOnHide is false, so their form state survives a switch", () => {
    const w = mount(harness({ unmountOnHide: false }));
    expect(w.find("#in-a").exists()).toBe(true);
    expect(w.find("#in-b").exists()).toBe(true);

    const panels = w.findAll('[role="tabpanel"]');
    expect(panels[0].attributes("hidden")).toBeUndefined();
    expect(panels[1].attributes("hidden")).toBeDefined();
  });

  it("unmounts inactive panels by default, pinning reka's own default through the wrapper", () => {
    const w = mount(harness());
    expect(w.find("#in-a").exists()).toBe(true);
    expect(w.find("#in-b").exists()).toBe(false);
  });

  it("wires each trigger to its panel once reka's item collection has registered", async () => {
    const w = mount(harness({ unmountOnHide: false }));
    await nextTick();

    const trigger = w.findAll('[role="tab"]')[0];
    const panel = w.findAll('[role="tabpanel"]')[0];
    expect(trigger.attributes("aria-controls")).toBe(panel.attributes("id"));
    expect(panel.attributes("aria-labelledby")).toBe(trigger.attributes("id"));
  });

  it("leaves the strip reachable by keyboard", async () => {
    const w = mount(harness({ unmountOnHide: false }));
    await nextTick();

    expect(w.find('[role="tablist"]').attributes("tabindex")).toBe("0");
  });
});

// The PHP page's version rail is vertical. Its layout comes entirely from
// `data-orientation` reaching the list and triggers through reka's
// RovingFocusGroup/Slot attribute merge, and its keyboard nav switches to the
// vertical arrow keys - neither is visible in the markup we author.
describe("ui/tabs vertical orientation", () => {
  it("puts data-orientation on the list and its triggers", () => {
    const w = mount(harness({ orientation: "vertical", unmountOnHide: false }));

    expect(w.find('[role="tablist"]').attributes("data-orientation")).toBe("vertical");
    for (const trigger of w.findAll('[role="tab"]')) {
      expect(trigger.attributes("data-orientation")).toBe("vertical");
    }
    expect(w.find('[role="tablist"]').attributes("aria-orientation")).toBe("vertical");
  });

  it("moves selection with ArrowDown and ArrowUp, wrapping at both ends", async () => {
    const w = mount(harness({ orientation: "vertical", unmountOnHide: false }), {
      attachTo: document.body,
    });
    await nextTick();

    const tabs = () => w.findAll('[role="tab"]');
    const selected = () =>
      tabs().findIndex((t) => t.attributes("aria-selected") === "true");
    expect(selected()).toBe(0);

    await tabs()[0].trigger("keydown", { key: "ArrowDown" });
    expect(selected()).toBe(1);

    await tabs()[1].trigger("keydown", { key: "ArrowDown" });
    expect(selected()).toBe(0);

    await tabs()[0].trigger("keydown", { key: "ArrowUp" });
    expect(selected()).toBe(1);
  });

  it("ignores the horizontal arrow keys when vertical", async () => {
    const w = mount(harness({ orientation: "vertical", unmountOnHide: false }), {
      attachTo: document.body,
    });
    await nextTick();

    await w.findAll('[role="tab"]')[0].trigger("keydown", { key: "ArrowRight" });
    expect(w.findAll('[role="tab"]')[0].attributes("aria-selected")).toBe("true");
  });
});
