import { describe, expect, it } from "vitest";

import { effectiveValue, overrideCount, SETTING_KEYS } from "./phpSettings";

describe("effectiveValue", () => {
  const global = { memory_limit: "512M", max_execution_time: "60" };

  it("prefers the per-version override", () => {
    expect(effectiveValue(global, { memory_limit: "1G" }, "memory_limit")).toBe("1G");
  });

  it("falls back to the global value", () => {
    expect(effectiveValue(global, {}, "memory_limit")).toBe("512M");
  });

  it("treats an empty override as inherit", () => {
    expect(effectiveValue(global, { memory_limit: "" }, "memory_limit")).toBe("512M");
  });

  it("is undefined when neither layer sets the key (PHP default)", () => {
    expect(effectiveValue(global, {}, "display_errors")).toBeUndefined();
  });
});

describe("overrideCount", () => {
  it("counts only non-empty allowlisted overrides", () => {
    expect(overrideCount({})).toBe(0);
    expect(
      overrideCount({ memory_limit: "1G", display_errors: "Off", max_input_time: "" }),
    ).toBe(2);
    expect(overrideCount({ not_a_setting: "x" })).toBe(0);
  });

  it("SETTING_KEYS covers the 8 allowlisted settings", () => {
    expect(SETTING_KEYS).toHaveLength(8);
    expect(SETTING_KEYS).toContain("display_errors");
  });
});
