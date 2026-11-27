import { describe, expect, it } from "vitest";

import {
  changedCount,
  directiveNameProblem,
  displayErrorsOptions,
  displayErrorsPick,
  GRID_ORDER,
  poolMaxChildrenProblem,
  directiveValueProblem,
  effectiveValue,
  overrideCount,
  POOL_MAX_CHILDREN_KEY,
  SETTING_KEYS,
  XDEBUG_DIRECTIVES_HINT,
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

  it("SETTING_KEYS covers the 9 allowlisted settings", () => {
    expect(SETTING_KEYS).toHaveLength(9);
    expect(SETTING_KEYS).toContain("display_errors");
    expect(SETTING_KEYS).toContain("ffi.enable");
  });
});

describe("directiveNameProblem", () => {
  it("accepts real extension directive names", () => {
    for (const name of ["xdebug.mode", "opcache.enable", "zend.assertions", "_x"]) {
      expect(directiveNameProblem(name)).toBeNull();
    }
  });

  it("flags allowlisted settings and reserved names", () => {
    expect(directiveNameProblem("memory_limit")).toMatch(/Settings grid/);
    expect(directiveNameProblem("extension")).toMatch(/extensions/);
    expect(directiveNameProblem("openssl.cafile")).toMatch(/CA bundle/);
  });

  it("points ffi.enable at the FFI setting", () => {
    expect(directiveNameProblem("ffi.enable")).toMatch(/FFI in web requests setting/);
  });

  it("treats Object.prototype member names as ordinary directives, not inherited hints", () => {
    for (const name of ["constructor", "toString", "valueOf", "hasOwnProperty"]) {
      expect(directiveNameProblem(name)).toBeNull();
    }
  });

  it("flags malformed names", () => {
    for (const name of ["", "1st", ".dot", "has space", "semi;colon"]) {
      expect(directiveNameProblem(name)).not.toBeNull();
    }
  });

  it("flags FPM pool settings, which are not ini directives", () => {
    for (const name of ["pm.max_children", "pm.start_servers", "pm.max_requests"]) {
      expect(directiveNameProblem(name)).toMatch(/FPM workers/);
    }
  });

  it("does not flag names that merely start with pm", () => {
    for (const name of ["pm", "pmx", "pm_max_children"]) {
      expect(directiveNameProblem(name)).toBeNull();
    }
  });
});

describe("poolMaxChildrenProblem", () => {
  it("accepts the bounds and an empty field, which means the default", () => {
    for (const value of ["", "  ", "1", "16", "32", "1024", " 64 "]) {
      expect(poolMaxChildrenProblem(value)).toBeNull();
    }
  });

  it("flags out-of-range values", () => {
    for (const value of ["0", "1025", "99999"]) {
      expect(poolMaxChildrenProblem(value)).toMatch(/between 1 and 1024/);
    }
  });

  it("flags anything that is not a whole number", () => {
    for (const value of ["abc", "16.5", "-1", "+8", "1e3"]) {
      expect(poolMaxChildrenProblem(value)).toMatch(/whole number/);
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

describe("GRID_ORDER", () => {
  it("lists every allowlisted setting once plus the worker ceiling", () => {
    const settings = GRID_ORDER.filter((k) => k !== POOL_MAX_CHILDREN_KEY);
    expect([...settings].sort()).toEqual([...SETTING_KEYS].sort());
    expect(new Set(GRID_ORDER).size).toBe(GRID_ORDER.length);
    expect(GRID_ORDER).toContain(POOL_MAX_CHILDREN_KEY);
  });
});

describe("changedCount", () => {
  it.each([
    ["nothing staged", {}, "", true, 0],
    ["two settings", { memory_limit: "1G", display_errors: "On" }, "", true, 2],
    ["blank and whitespace values", { memory_limit: "", max_input_time: "  " }, "", true, 0],
    ["the worker ceiling", {}, "32", true, 1],
    ["settings and the ceiling", { memory_limit: "1G" }, "32", true, 2],
    ["a visible FFI override", { "ffi.enable": "On" }, "", true, 1],
    ["a hidden FFI override", { "ffi.enable": "On", memory_limit: "1G" }, "", false, 1],
    ["an unknown key", { not_a_setting: "x" }, "", true, 0],
  ] as const)("counts %s", (_name, form, pool, ffi, expected) => {
    expect(changedCount({ ...form }, pool, ffi)).toBe(expected);
  });
});

describe("displayErrorsOptions", () => {
  it("offers only On and Off when a value is inherited", () => {
    expect(displayErrorsOptions("On").map((o) => o.value)).toEqual(["On", "Off"]);
  });

  it("adds Default when nothing is inherited", () => {
    expect(displayErrorsOptions(undefined).map((o) => o.value)).toEqual(["", "On", "Off"]);
  });
});

describe("displayErrorsPick", () => {
  it.each([
    ["the inherited value means inherit", "On", "On", ""],
    ["another value is an override", "Off", "On", "Off"],
    ["any value without an inherited one", "On", undefined, "On"],
    ["Default without an inherited one", "", undefined, ""],
  ] as const)("%s", (_name, picked, inherited, expected) => {
    expect(displayErrorsPick(picked, inherited)).toBe(expected);
  });
});

describe("XDEBUG_DIRECTIVES_HINT", () => {
  it("names the Xdebug section without pointing at a position", () => {
    expect(XDEBUG_DIRECTIVES_HINT).toContain("the Xdebug section sets");
    expect(XDEBUG_DIRECTIVES_HINT).not.toContain("above");
  });
});
