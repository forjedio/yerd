import { afterEach, describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";

import SiteFilterChips from "./SiteFilterChips.vue";
import type { SiteChip } from "@/lib/sites";

const chips: SiteChip[] = [
  { key: "all", label: "All", count: 5 },
  { key: "https", label: "HTTPS", count: 2 },
  { key: "linked", label: "Linked", count: 1 },
];

const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetTop");

function layoutRows(perChild: (index: number) => number): void {
  Object.defineProperty(HTMLElement.prototype, "offsetTop", {
    configurable: true,
    get(this: HTMLElement) {
      const parent = this.parentElement;
      return parent ? perChild(Array.from(parent.children).indexOf(this)) : 0;
    },
  });
}

afterEach(() => {
  if (original) Object.defineProperty(HTMLElement.prototype, "offsetTop", original);
  document.body.innerHTML = "";
});

function mountChips() {
  return mount(SiteFilterChips, {
    attachTo: document.body,
    props: { chips, modelValue: "https", query: "" },
  });
}

describe("SiteFilterChips", () => {
  it("shows the pills when they fit on two rows", async () => {
    layoutRows((i) => (i < 2 ? 0 : 32));
    const w = mountChips();
    await flushPromises();
    expect(w.find('[data-testid="chips-dropdown"]').exists()).toBe(false);
    expect(w.findAll('button[data-chip]')).toHaveLength(3);
    await w.get('button[data-chip="linked"]').trigger("click");
    expect(w.emitted("update:modelValue")?.[0]).toEqual(["linked"]);
  });

  it("collapses into a dropdown showing the active chip past two rows", async () => {
    layoutRows((i) => i * 32);
    const w = mountChips();
    await flushPromises();
    const trigger = w.get('[data-testid="chips-dropdown"]');
    expect(trigger.text()).toContain("HTTPS");
    expect(trigger.text()).toContain("2");
    expect(w.findAll('button[data-chip]')).toHaveLength(0);
  });
});
