import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ComingSoon from "./ComingSoon.vue";
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
