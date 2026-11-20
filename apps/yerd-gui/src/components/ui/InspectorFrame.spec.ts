import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";

import InspectorFrame from "./InspectorFrame.vue";

function escape(target: EventTarget = document): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

function mountDrawer(layerOpen?: () => boolean) {
  return mount(InspectorFrame, {
    attachTo: document.body,
    props: { mode: "drawer", shown: true, label: "PHP 8.4", testid: "php-inspector", layerOpen },
  });
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("InspectorFrame Escape", () => {
  it("closes an open drawer and marks the Escape handled", () => {
    const w = mountDrawer();
    const event = escape();
    expect(w.emitted("close")).toHaveLength(1);
    expect(event.defaultPrevented).toBe(true);
    w.unmount();
  });

  it("leaves Escape to an open dismissable layer such as a tooltip", () => {
    const w = mountDrawer();
    const tooltip = document.createElement("div");
    tooltip.setAttribute("data-dismissable-layer", "");
    document.body.append(tooltip);
    const event = escape();
    expect(w.emitted("close")).toBeUndefined();
    expect(event.defaultPrevented).toBe(false);
    w.unmount();
  });

  it("leaves Escape alone once something else default-prevented it", () => {
    const w = mountDrawer();
    const claim = (event: Event) => event.preventDefault();
    document.body.addEventListener("keydown", claim);
    escape(document.body);
    expect(w.emitted("close")).toBeUndefined();
    document.body.removeEventListener("keydown", claim);
    w.unmount();
  });

  it("defers to a caller-reported layer", () => {
    const w = mountDrawer(() => true);
    escape();
    expect(w.emitted("close")).toBeUndefined();
    w.unmount();
  });

  it("lets a discard prompt opened by the close survive the same keypress", async () => {
    const promptOpen = ref(false);
    const Host = defineComponent({
      setup() {
        return () =>
          h(InspectorFrame, {
            mode: "drawer",
            shown: true,
            label: "PHP 8.4",
            testid: "php-inspector",
            onClose: () => {
              promptOpen.value = true;
            },
          });
      },
    });
    const w = mount(Host, { attachTo: document.body });
    const dismissPrompt = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) promptOpen.value = false;
    };
    window.addEventListener("keydown", dismissPrompt);
    escape();
    await nextTick();
    expect(promptOpen.value).toBe(true);
    window.removeEventListener("keydown", dismissPrompt);
    w.unmount();
  });
});
