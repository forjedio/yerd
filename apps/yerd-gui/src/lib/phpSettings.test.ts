import { describe, expect, it } from "vitest";

import {
  directiveNameProblem,
  directiveValueProblem,
  effectiveValue,
  overrideCount,
  SETTING_KEYS,
} from "./phpSettings";

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

describe("directiveNameProblem", () => {
  it("accepts real extension directive names", () => {
    for (const name of ["xdebug.mode", "opcache.enable", "zend.assertions", "_x"]) {
      expect(directiveNameProblem(name)).toBeNull();
    }
  });

  it("flags allowlisted settings and reserved names", () => {
    expect(directiveNameProblem("memory_limit")).toMatch(/settings form/);
    expect(directiveNameProblem("extension")).toMatch(/extensions/);
    expect(directiveNameProblem("openssl.cafile")).toMatch(/CA bundle/);
  });

  // An object-literal lookup would resolve these up the prototype chain and
  // hand back a function where a hint string is expected.
  it("treats Object.prototype member names as ordinary directives", () => {
    for (const name of ["constructor", "toString", "valueOf", "hasOwnProperty"]) {
      expect(directiveNameProblem(name)).toBeNull();
    }
  });

  it("flags malformed names", () => {
    for (const name of ["", "1st", ".dot", "has space", "semi;colon"]) {
      expect(directiveNameProblem(name)).not.toBeNull();
    }
  });
});

describe("directiveValueProblem", () => {
  it("accepts ordinary values", () => {
    for (const value of ["debug", "develop,debug", "1", "256M", "/a/b c.log"]) {
      expect(directiveValueProblem(value)).toBeNull();
    }
  });

  it("flags empty values and injection metacharacters", () => {
    for (const value of ["", "  ", "a;b", "a#b", "a=b", "a[b", "a]b", "a\nb"]) {
      expect(directiveValueProblem(value)).not.toBeNull();
    }
  });
});
