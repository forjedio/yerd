import { describe, expect, it } from "vitest";

import type { Diagnosis, DiagnosisCode, Severity } from "@/ipc/types";
import {
  defaultSelection,
  findingGroups,
  hasActionable,
  permissionCopy,
  permissionKey,
  SEVERITY_CHIP,
} from "./doctor";
import type { PrivilegeItem } from "./elevation";

function d(code: DiagnosisCode, severity: Severity, title: string = code): Diagnosis {
  return { code, severity, title, detail: "", remedy: null };
}

function perm(over: Partial<PrivilegeItem>): PrivilegeItem {
  return {
    key: "trust",
    label: "Local CA trusted",
    value: true,
    fixable: false,
    unelevatable: true,
    target: "trust",
    yes: "trusted",
    no: "not trusted",
    ...over,
  };
}

describe("findingGroups", () => {
  it("orders Problems, Warnings, Info, keeps daemon order and drops all_good", () => {
    const groups = findingGroups([
      d("php_update_available", "ok"),
      d("bin_dir_not_on_path", "warn"),
      d("fpm_pool_failed", "fail"),
      d("domain_shadowed", "warn"),
      d("all_good", "ok"),
    ]);
    expect(groups.map((g) => [g.label, g.items.map((i) => i.finding.code)])).toEqual([
      ["Problems", ["fpm_pool_failed"]],
      ["Warnings", ["bin_dir_not_on_path", "domain_shadowed"]],
      ["Info", ["php_update_available"]],
    ]);
  });

  it.each([
    ["no findings", []],
    ["only all_good", [d("all_good", "ok")]],
  ])("is empty for %s", (_label, ds) => {
    expect(findingGroups(ds)).toEqual([]);
  });

  it("keys repeated codes by title and repeated titles by occurrence", () => {
    const keys = findingGroups([
      d("service_failed", "fail", "MySQL failed"),
      d("service_failed", "fail", "Redis failed"),
      d("service_failed", "fail", "Redis failed"),
    ]).flatMap((g) => g.items.map((i) => i.key));
    expect(keys).toEqual([
      "service_failed:MySQL failed",
      "service_failed:Redis failed",
      "service_failed:Redis failed#1",
    ]);
  });
});

describe("defaultSelection", () => {
  it.each([
    ["first problem", [d("bin_dir_not_on_path", "warn"), d("fpm_pool_failed", "fail")], "fpm_pool_failed:fpm_pool_failed"],
    ["first warning when no problem", [d("no_sites", "ok"), d("bin_dir_not_on_path", "warn")], "bin_dir_not_on_path:bin_dir_not_on_path"],
    ["nothing when only info", [d("no_sites", "ok")], null],
    ["nothing when empty", [], null],
  ])("%s", (_label, ds, expected) => {
    expect(defaultSelection(findingGroups(ds))).toBe(expected);
  });
});

describe("hasActionable and SEVERITY_CHIP", () => {
  it.each([
    [[d("no_sites", "ok")], false],
    [[d("no_sites", "ok"), d("bin_dir_not_on_path", "warn")], true],
    [[d("fpm_pool_failed", "fail")], true],
  ])("hasActionable %#", (ds, expected) => {
    expect(hasActionable(ds)).toBe(expected);
  });

  it("labels each severity", () => {
    expect(SEVERITY_CHIP).toEqual({ fail: "Problem", warn: "Warning", ok: "Info" });
  });
});

describe("permissionCopy", () => {
  it.each([
    ["granted and revertible", perm({}), "Trusted", "Trusted", "Granted", /Revert removes it/],
    ["granted, not revertible", perm({ unelevatable: false }), "Trusted", "Trusted", "Granted", /Nothing to do/],
    ["not granted", perm({ value: false, fixable: true, unelevatable: false }), "Not trusted", "Not trusted", "Needs your password", /password/],
    ["unknown", perm({ value: null, fixable: true, unelevatable: false }), "Unknown", "Unknown", "Status unknown", /couldn't read/],
    [
      "not granted and not fixable yet",
      perm({ target: "ports", value: false, fixable: false, unelevatable: false, no: "not serving - set working ports first" }),
      "Not serving - set working ports first",
      "Not serving",
      "Not available yet",
      /can't fix/,
    ],
  ])("%s", (_label, item, summary, chip, cardTitle, body) => {
    const copy = permissionCopy(item);
    expect(copy).toMatchObject({ summary, chip, cardTitle });
    expect(copy.cardBody).toMatch(body);
  });

  it.each([["trust"], ["resolver"], ["ports"]] as const)("describes %s", (target) => {
    expect(permissionCopy(perm({ target })).description).not.toBe("");
  });

  it("keys permission rows by target", () => {
    expect(permissionKey("resolver")).toBe("perm:resolver");
  });
});
