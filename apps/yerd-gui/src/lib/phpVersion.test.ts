import { describe, expect, it } from "vitest";

import { comparePhpVersions, isLegacyVersion, phpVersionInRange } from "./phpVersion";

describe("comparePhpVersions", () => {
  it.each([
    ["8.3", "8.3", 0],
    ["8.4", "8.3", 1],
    ["8.3", "8.4", -1],
    ["7.4", "8.0", -1],
    ["8.0", "7.4", 1],
    // Numeric, not lexicographic: a plain string compare would put "8.10"
    // before "8.3" and "8.9" after "8.10".
    ["8.3", "8.10", -1],
    ["8.10", "8.9", 1],
    ["8.9", "8.10", -1],
  ])("comparePhpVersions(%s, %s) has the sign of %i", (a, b, expectedSign) => {
    const result = comparePhpVersions(a, b);
    expect(Math.sign(result)).toBe(expectedSign);
  });
});

describe("isLegacyVersion", () => {
  it.each([
    ["7.4", true],
    ["8.0", true],
    ["8.1", true],
    ["8.2", false],
    ["8.3", false],
    ["8.5", false],
    ["8.10", false],
  ] as const)("isLegacyVersion(%s) === %s", (php, expected) => {
    expect(isLegacyVersion(php)).toBe(expected);
  });
});

describe("phpVersionInRange", () => {
  it.each([
    // [php, min, max, expected]
    ["7.3", "7.3", "8.4", true], // at the min boundary, inclusive
    ["8.4", "7.3", "8.4", true], // at the max boundary, inclusive
    ["8.0", "7.3", "8.4", true], // comfortably inside
    ["7.2", "7.3", "8.4", false], // just below the floor
    ["8.5", "7.3", "8.4", false], // just above the ceiling
    ["8.10", "7.3", "8.4", false], // numerically above 8.4, would wrongly pass a lexicographic compare
    ["8.3", "7.3", "8.10", true], // numerically below 8.10, would wrongly fail a lexicographic compare
  ])("phpVersionInRange(%s, %s, %s) === %s", (php, min, max, expected) => {
    expect(phpVersionInRange(php, min, max)).toBe(expected);
  });

  it("excludes a malformed (non major.minor) version rather than matching everything", () => {
    // PhpVersion always arrives as "major.minor" from the daemon, but this
    // documents the safe failure mode if that guarantee is ever violated. A
    // bare major only produces NaN once the minor comparison is actually
    // reached (i.e. the majors are equal) - comparePhpVersions short-circuits
    // on a differing major before ever looking at minor, so "8" vs a range
    // with a different major (e.g. 7.0-9.0) would misleadingly still compare
    // cleanly. Pin the case that actually exercises the NaN path: NaN is
    // never >= 0 or <= 0, so a same-major, malformed comparison excludes
    // rather than crashing or silently matching.
    expect(phpVersionInRange("8", "8.0", "8.5")).toBe(false);
  });
});
