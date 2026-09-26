import { describe, expect, it } from "vitest";
import { reactive } from "vue";

import { useSiteBasics } from "./useSiteBasics";

function basics(name: string) {
  const form = reactive({ name, location: "/srv", php: "8.4", secure: false });
  return useSiteBasics(form, { tld: "test", report: null });
}

describe("useSiteBasics", () => {
  it.each([
    ["blog", true],
    ["My-Blog", true],
    ["  blog  ", true],
    ["-blog", false],
    ["blog-", false],
    ["my_blog", false],
    ["a".repeat(64), false],
    ["Kelvin", false],
  ])("validates %j as %s", (name, expected) => {
    expect(basics(name).nameValid.value).toBe(expected);
  });

  it("derives the lowercased domain and folder the daemon creates", () => {
    const b = basics(" My-Blog ");
    expect(b.domain.value).toBe("my-blog.test");
    expect(b.projectPath.value).toBe("/srv/my-blog");
    expect(b.basicsValid.value).toBe(true);
  });
});
