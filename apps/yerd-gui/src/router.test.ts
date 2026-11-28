import { describe, expect, it } from "vitest";

import { router } from "./router";

describe("router", () => {
  it("redirects the old Dumps page to Insights", () => {
    const matched = router.resolve("/dumps").matched;

    expect(matched).toHaveLength(1);
    expect(matched[0]?.redirect).toBe("/insights");
  });

  it("serves Insights at /insights", () => {
    expect(router.resolve("/insights").name).toBe("insights");
  });

  it("keeps the pop-out Live feed window route", () => {
    expect(router.resolve("/dumps-window").name).toBe("dumps-window");
  });
});
