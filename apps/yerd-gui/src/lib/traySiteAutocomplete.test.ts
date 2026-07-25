import { describe, expect, it } from "vitest";

import {
  buildTraySiteSuggestions,
  pushRecent,
  siteMatches,
  toggleFavorite,
} from "./traySiteAutocomplete";
import type { SiteEntry } from "@/ipc/types";

function site(over: Partial<SiteEntry> & Pick<SiteEntry, "name">): SiteEntry {
  return {
    kind: "linked",
    document_root: `/Users/dev/${over.name}`,
    php: "8.4",
    secure: true,
    ...over,
  } as SiteEntry;
}

describe("siteMatches", () => {
  it("matches name, domain, and path segments", () => {
    const s = site({
      name: "blog",
      primary_domain: "blog.example.test",
      document_root: "/Users/dev/Projects/my-blog",
    });
    expect(siteMatches(s, "blog", "test")).toBe(true);
    expect(siteMatches(s, "example", "test")).toBe(true);
    expect(siteMatches(s, "projects", "test")).toBe(true);
    expect(siteMatches(s, "zzz", "test")).toBe(false);
  });
});

describe("buildTraySiteSuggestions", () => {
  const sites = [
    site({ name: "alpha" }),
    site({ name: "beta" }),
    site({ name: "gamma", document_root: "/tmp/gamma-app" }),
  ];

  it("groups favorites and recent when query is empty", () => {
    const out = buildTraySiteSuggestions(sites, "", {
      favorites: ["beta"],
      recent: ["gamma"],
      tld: "test",
      emptyCap: 8,
    });
    expect(out[0]?.group).toBe("Favorites");
    expect(out[0]?.site.name).toBe("beta");
    expect(out[1]?.group).toBe("Recent");
    expect(out[1]?.site.name).toBe("gamma");
  });

  it("filters by query and boosts favorites", () => {
    const out = buildTraySiteSuggestions(sites, "a", {
      favorites: ["gamma"],
      recent: [],
      tld: "test",
    });
    expect(out.every((s) => siteMatches(s.site, "a", "test"))).toBe(true);
    expect(out[0]?.site.name).toBe("gamma");
  });

  it("returns empty for no match", () => {
    expect(
      buildTraySiteSuggestions(sites, "zzzz", {
        favorites: [],
        recent: [],
        tld: "test",
      }),
    ).toEqual([]);
  });
});

describe("pushRecent / toggleFavorite", () => {
  it("pushes MRU and caps", () => {
    expect(pushRecent(["a", "b"], "c", 2)).toEqual(["c", "a"]);
    expect(pushRecent(["a", "b"], "a", 8)).toEqual(["a", "b"]);
  });

  it("toggles favorites", () => {
    expect(toggleFavorite(["a"], "b")).toEqual(["a", "b"]);
    expect(toggleFavorite(["a", "b"], "a")).toEqual(["b"]);
  });
});
