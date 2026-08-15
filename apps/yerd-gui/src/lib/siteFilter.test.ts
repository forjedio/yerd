import { describe, expect, it } from "vitest";

import { matchesSiteFilter } from "./siteFilter";
import type { SiteEntry } from "@/ipc/types";

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "codestash",
    document_root: "/srv/codestash",
    php: "8.3",
    secure: false,
    kind: "linked",
    ...overrides,
  } as SiteEntry;
}

describe("matchesSiteFilter", () => {
  const cases: { query: string; expected: boolean; why: string }[] = [
    { query: "", expected: true, why: "an empty query matches everything" },
    { query: "   ", expected: true, why: "a whitespace-only query is empty" },
    { query: "codestash", expected: true, why: "the apex label" },
    { query: "CODESTASH.TEST", expected: true, why: "matching is case-insensitive" },
    { query: "admin.", expected: true, why: "an added subdomain" },
    { query: "shop.example.test", expected: true, why: "a whole added domain" },
    { query: "*.staging", expected: true, why: "a wildcard domain" },
    { query: "blog", expected: false, why: "no domain contains it" },
  ];

  const configured = site({
    primary_domain: "codestash.test",
    domains: ["codestash.test", "admin.codestash.test", "shop.example.test", "*.staging.test"],
  });

  for (const { query, expected, why } of cases) {
    it(`${expected ? "matches" : "rejects"} ${JSON.stringify(query)} - ${why}`, () => {
      expect(matchesSiteFilter(configured, "test", query)).toBe(expected);
    });
  }

  it("falls back to the default apex for a site with no configured domains", () => {
    expect(matchesSiteFilter(site(), "test", "codestash.test")).toBe(true);
    expect(matchesSiteFilter(site(), "test", "admin.")).toBe(false);
  });

  it("keeps the default apex findable even when domains omit it", () => {
    const moved = site({ primary_domain: "example.test", domains: ["example.test"] });
    expect(matchesSiteFilter(moved, "test", "codestash.test")).toBe(true);
    expect(matchesSiteFilter(moved, "test", "example")).toBe(true);
  });
});
