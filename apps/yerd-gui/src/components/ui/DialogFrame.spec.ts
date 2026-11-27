import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import { useToast } from "@/composables/useToast";

import DialogFrame from "./DialogFrame.vue";
import Toaster from "./Toaster.vue";
import { isToasterInteraction, TOASTER_ATTR } from "./toaster";

function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function pointerDown(target: Element): void {
  target.dispatchEvent(new Event("pointerdown", { bubbles: true }));
}

afterEach(() => {
  const { toasts, dismiss } = useToast();
  for (const t of [...toasts.value]) dismiss(t.id);
  document.body.innerHTML = "";
});

describe("toaster interaction", () => {
  it("recognises events whose original target is inside the toaster", () => {
    const toaster = document.createElement("div");
    toaster.setAttribute(TOASTER_ATTR, "");
    const button = document.createElement("button");
    toaster.append(button);
    const outside = document.createElement("div");
    document.body.append(toaster, outside);

    const inside = new CustomEvent("x", { detail: { originalEvent: { target: button } } });
    const other = new CustomEvent("x", { detail: { originalEvent: { target: outside } } });
    expect(isToasterInteraction(inside)).toBe(true);
    expect(isToasterInteraction(other)).toBe(false);
  });

  it("keeps the toast viewport clickable over a modal", () => {
    const w = mount(Toaster, { attachTo: document.body });
    const viewport = document.querySelector(`[${TOASTER_ATTR}]`);
    expect(viewport?.classList.contains("pointer-events-auto")).toBe(true);
    w.unmount();
  });
});

describe("DialogFrame", () => {
  it("stays open when a toast is clicked but closes on a real outside click", async () => {
    const toaster = mount(Toaster, { attachTo: document.body });
    useToast().error("Install failed", "boom");
    await flushPromises();
    const dialog = mount(DialogFrame, {
      attachTo: document.body,
      props: { open: true, title: "New site" },
    });
    await flushPromises();
    await settle();
    await settle();

    const dismissButton = document.querySelector(`[${TOASTER_ATTR}] button`);
    expect(dismissButton).not.toBeNull();
    pointerDown(dismissButton!);
    await settle();
    await flushPromises();
    expect(dialog.emitted("update:open")).toBeUndefined();

    const outside = document.createElement("div");
    document.body.append(outside);
    pointerDown(outside);
    await settle();
    await flushPromises();
    expect(dialog.emitted("update:open")).toEqual([[false]]);

    dialog.unmount();
    toaster.unmount();
  });
});
