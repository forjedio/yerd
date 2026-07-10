import { describe, expect, it } from "vitest";

import { slugifySiteName } from "./siteName";

describe("slugifySiteName", () => {
  it("mirrors the Rust slugify_site_name cases", () => {
    const cases: [string, string | null][] = [
      ["My Project", "my-project"],
      ["my_app", "my-app"],
      ["example.com", "example-com"],
      ["ex.com", "ex-com"],
      ["a..b", "a-b"],
      ["-leading", "leading"],
      ["trailing-", "trailing"],
      ["already-valid", "already-valid"],
      ["???", null],
      ["", null],
    ];
    for (const [input, expected] of cases) {
      expect(slugifySiteName(input), `input ${JSON.stringify(input)}`).toBe(expected);
    }
  });

  it("treats non-ASCII letters as separators, mirroring Rust is_ascii_alphanumeric", () => {
    const cases: [string, string | null][] = [
      ["café", "caf"],
      ["a①b", "a-b"],
      ["naïve", "na-ve"],
      ["ﬀ", null],
    ];
    for (const [input, expected] of cases) {
      expect(slugifySiteName(input), `input ${JSON.stringify(input)}`).toBe(expected);
    }
  });

  it("caps at 63 chars without a trailing hyphen", () => {
    const slug = slugifySiteName("a".repeat(65));
    expect(slug).not.toBeNull();
    expect(slug?.length).toBe(63);
    expect(slug?.endsWith("-")).toBe(false);
  });

  it("does not leave a dangling hyphen when a separator lands on the boundary", () => {
    const slug = slugifySiteName(`${"a".repeat(62)} ${"b".repeat(5)}`);
    expect(slug?.length).toBe(62);
    expect(slug?.endsWith("-")).toBe(false);
  });
});
