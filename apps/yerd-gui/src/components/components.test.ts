import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ComingSoon from "./ComingSoon.vue";
import SegmentedControl from "./ui/SegmentedControl.vue";
import StatusPill from "./StatusPill.vue";

describe("ComingSoon", () => {
  it("is non-interactive and explains why", () => {
    const w = mount(ComingSoon, {
      props: { reason: "needs a daemon Logs IPC" },
      slots: { default: "Logs" },
    });
    // aria-disabled + a hover explanation; no button/clickable element.
    expect(w.attributes("aria-disabled")).toBe("true");
    expect(w.attributes("title")).toContain("Logs IPC");
    expect(w.find("button").exists()).toBe(false);
    expect(w.text()).toContain("soon");
  });
});

describe("StatusPill tri-state tones", () => {
  it.each([
    ["ok", "bg-success"],
    ["bad", "bg-destructive"],
    ["unknown", "bg-muted-foreground"],
  ] as const)("tone %s uses the %s dot", (tone, klass) => {
    const w = mount(StatusPill, { props: { tone, label: tone } });
    expect(w.html()).toContain(klass);
    expect(w.text()).toContain(tone);
  });
});

describe("SegmentedControl", () => {
  const options = [
    { value: "8.3", label: "8.3" },
    { value: "8.4", label: "8.4" },
  ];

  it("renders each option and marks the selected one", () => {
    const w = mount(SegmentedControl, {
      props: { modelValue: "8.4", options, ariaLabel: "PHP version" },
    });
    const radios = w.findAll('[role="radio"]');
    expect(radios.map((r) => r.text())).toEqual(["8.3", "8.4"]);
    expect(radios[1]?.attributes("aria-checked")).toBe("true");
  });

  it("emits the clicked value", async () => {
    const w = mount(SegmentedControl, {
      props: { modelValue: "8.4", options, ariaLabel: "PHP version" },
    });
    await w.findAll('[role="radio"]')[0]?.trigger("click");
    expect(w.emitted("update:modelValue")).toEqual([["8.3"]]);
  });

  it("emits nothing while disabled", async () => {
    const w = mount(SegmentedControl, {
      props: { modelValue: "8.4", options, ariaLabel: "PHP version", disabled: true },
    });
    await w.findAll('[role="radio"]')[0]?.trigger("click");
    expect(w.emitted("update:modelValue")).toBeUndefined();
  });
});
