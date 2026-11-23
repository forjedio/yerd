import { describe, expect, it } from "vitest";

import {
  aliasCount,
  buildSections,
  frameworkDetail,
  frameworkLabel,
  groupNameAvailable,
  httpsDetail,
  matchesChip,
  normalizeDomain,
  OVERFLOW_LIMIT,
  servedPath,
  servingState,
  siteAtPath,
  siteChips,
  siteNameTaken,
  siteWarnings,
  sourceDetail,
  tildify,
  untildify,
  urlParts,
  visibleRows,
  type SiteSection,
} from "./sites";
import type { GroupsState, ProxyEntry, SiteEntry, StatusReport } from "@/ipc/types";

function site(name: string, overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name,
    document_root: `/home/a/Herd/${name}`,
    php: "8.4",
    secure: false,
    kind: "parked",
    ...overrides,
  };
}

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ca: { path: "", fingerprint: "", trusted_system: true },
    ...overrides,
  } as StatusReport;
}

const NO_GROUPS: GroupsState = { order: [], members: {} };

describe("siteChips", () => {
  it("counts every chip and orders frameworks by size", () => {
    const sites = [
      site("a", { secure: true, framework: "laravel" }),
      site("b", { framework: "laravel", php: "8.3" }),
      site("c", { kind: "linked", framework: "wordpress" }),
      site("d", { framework: "other" }),
    ];
    expect(siteChips(sites, "8.4")).toEqual([
      { key: "all", label: "All", count: 4 },
      { key: "https", label: "HTTPS", count: 1 },
      { key: "linked", label: "Linked", count: 1 },
      { key: "framework:laravel", label: "Laravel", count: 2 },
      { key: "framework:wordpress", label: "WordPress", count: 1 },
      { key: "not-default", label: "Not on 8.4", count: 1 },
    ]);
  });

  it("drops zero-count chips but always keeps All", () => {
    expect(siteChips([], "8.4")).toEqual([{ key: "all", label: "All", count: 0 }]);
    expect(siteChips([site("a")], "8.4").map((c) => c.key)).toEqual(["all"]);
  });

  it("omits the default-PHP chip when no default is set", () => {
    const keys = siteChips([site("a", { php: "8.3" })], "").map((c) => c.key);
    expect(keys).not.toContain("not-default");
  });

  it.each([
    ["all", {}, true],
    ["https", { secure: true }, true],
    ["https", { secure: false }, false],
    ["linked", { kind: "linked" }, true],
    ["linked", {}, false],
    ["not-default", { php: "8.3" }, true],
    ["not-default", { php: "8.4" }, false],
    ["framework:laravel", { framework: "laravel" }, true],
    ["framework:laravel", { framework: "symfony" }, false],
  ] as const)("matchesChip %s with %o is %s", (key, overrides, expected) => {
    expect(matchesChip(site("x", overrides as Partial<SiteEntry>), key, "8.4")).toBe(expected);
  });
});

function keys(sections: SiteSection[]): string[] {
  return sections.map((s) => s.key);
}

function names(section: SiteSection | undefined): string[] {
  return (section?.sites ?? []).map((s) => s.name);
}

describe("siteAtPath", () => {
  const parked = site("foo", { document_root: "/code/foo" });
  const linked = site("acme", { kind: "linked", document_root: "/code/foo" });
  const other = site("blog", { kind: "linked", document_root: "/code/blog" });

  it.each([
    ["/code/foo", [parked, other], "foo"],
    ["/code/foo/", [parked, other], "foo"],
    ["/code/foo", [parked, linked, other], "acme"],
    ["/code/blog", [parked, linked, other], "blog"],
    ["/code/none", [parked, linked, other], null],
    ["/code/foo", [], null],
  ] as const)("%s in %j", (path, sites, want) => {
    expect(siteAtPath(sites, path)?.name ?? null).toBe(want);
  });
});

describe("buildSections", () => {
  const herd = "/home/a/Herd";
  const yerd = "/home/a/Yerd";

  it("orders groups, then parked folders, then Linked, then unmatched parked sites", () => {
    const sections = buildSections({
      sites: [
        site("zeta"),
        site("alpha"),
        site("y1", { document_root: `${yerd}/y1` }),
        site("lnk", { kind: "linked", document_root: "/elsewhere/lnk" }),
        site("odd", { document_root: "/private/Herd/odd" }),
        site("member"),
      ],
      groups: { order: ["Client Work"], members: { member: "client work" } },
      parked: [herd, yerd],
      filtering: false,
    });
    expect(keys(sections)).toEqual(["Client Work", `dir:${herd}`, `dir:${yerd}`, ":linked", ":parked"]);
    expect(sections[0]?.groupName).toBe("Client Work");
    expect(names(sections[0])).toEqual(["member"]);
    expect(names(sections[1])).toEqual(["alpha", "zeta"]);
    expect(sections[1]?.dir).toBe(herd);
    expect(names(sections[2])).toEqual(["y1"]);
    expect(names(sections[3])).toEqual(["lnk"]);
    expect(sections[4]?.label).toBe("Other parked sites");
    expect(names(sections[4])).toEqual(["odd"]);
  });

  it("puts a dangling membership back in its folder or Linked section", () => {
    const sections = buildSections({
      sites: [site("p"), site("l", { kind: "linked" })],
      groups: { order: [], members: { p: "Gone", l: "Gone" } },
      parked: [herd],
      filtering: false,
    });
    expect(names(sections.find((s) => s.key === `dir:${herd}`))).toEqual(["p"]);
    expect(names(sections.find((s) => s.key === ":linked"))).toEqual(["l"]);
  });

  it("keeps empty groups and folders while not filtering, and never empty Linked", () => {
    const sections = buildSections({
      sites: [],
      groups: { order: ["Empty"], members: {} },
      parked: [herd],
      filtering: false,
    });
    expect(keys(sections)).toEqual(["Empty", `dir:${herd}`]);
  });

  it("drops empty sections while filtering, whether by query or by chip alone", () => {
    const sections = buildSections({
      sites: [site("lnk", { kind: "linked" })],
      groups: { order: ["Empty"], members: {} },
      parked: [herd],
      filtering: true,
    });
    expect(keys(sections)).toEqual([":linked"]);
  });

  it("has no sections at all when nothing is parked, grouped or linked", () => {
    expect(buildSections({ sites: [], groups: NO_GROUPS, parked: [], filtering: false })).toEqual(
      [],
    );
  });
});

describe("visibleRows", () => {
  const many: SiteSection = {
    key: "k",
    kind: "dir",
    label: "k",
    sites: Array.from({ length: OVERFLOW_LIMIT + 3 }, (_, i) => site(`s${i}`)),
  };

  it("cuts a long section after the limit", () => {
    const { rows, hidden } = visibleRows(many, false, false);
    expect(rows).toHaveLength(OVERFLOW_LIMIT);
    expect(hidden).toBe(3);
  });

  it.each([
    [true, false],
    [false, true],
  ])("shows everything when expanded=%s or filtering=%s", (expanded, filtering) => {
    expect(visibleRows(many, expanded, filtering)).toEqual({ rows: many.sites, hidden: 0 });
  });

  it("leaves a short section alone", () => {
    const short = { ...many, sites: many.sites.slice(0, OVERFLOW_LIMIT) };
    expect(visibleRows(short, false, false).hidden).toBe(0);
  });
});

describe("groupNameAvailable", () => {
  const order = ["Client work", "Side"];
  it.each([
    ["New", undefined, true],
    ["  New  ", undefined, true],
    ["", undefined, false],
    ["   ", undefined, false],
    ["unallocated", undefined, false],
    ["CLIENT WORK", undefined, false],
    ["client WORK", "Client work", true],
    ["side", "Client work", false],
  ] as const)("%j (renaming %j) is %s", (name, except, expected) => {
    expect(groupNameAvailable(name, order, except)).toBe(expected);
  });
});

describe("siteWarnings", () => {
  const fellBack = {
    http: { requested: 80, bound: 8080, fell_back: true },
    https: { requested: 443, bound: 8443, fell_back: true },
  };

  it("is empty for a healthy report or no report", () => {
    expect(siteWarnings(report())).toEqual([]);
    expect(siteWarnings(null)).toEqual([]);
  });

  it("reports unbound web ports", () => {
    const [w] = siteWarnings(report({ web_unbound: { http: 8080, https: 8443 } }));
    expect(w).toEqual({
      key: "ports",
      text: "Yerd couldn't bind web ports 8080/8443, they're in use.",
      compactText: "Web ports 8080/8443 are busy.",
      action: "Run Doctor",
    });
  });

  it("names a foreign listener", () => {
    const [w] = siteWarnings(report({ ...fellBack, foreign_web_listener: true }));
    expect(w?.text).toContain("in use by another process");
    expect(w?.compactText).toBe("Ports 80/443 are busy — Yerd fell back to :8080 and :8443.");
  });

  it("says unforwarded otherwise, and nothing when a redirect carries the ports", () => {
    expect(siteWarnings(report(fellBack))[0]?.text).toContain("aren't forwarded");
    expect(siteWarnings(report(fellBack))[0]?.compactText).toBe(
      "Ports 80/443 aren't forwarded — Yerd is on :8080 and :8443.",
    );
    expect(siteWarnings(report({ ...fellBack, port_redirect: true }))).toEqual([]);
  });

  it("adds the CA strip only for an untrusted CA, after the ports strip", () => {
    const ca = (trusted: boolean | null) => ({ path: "", fingerprint: "", trusted_system: trusted });
    expect(siteWarnings(report({ ca: ca(null) }))).toEqual([]);
    const both = siteWarnings(report({ ...fellBack, ca: ca(false) }));
    expect(both.map((w) => [w.key, w.action])).toEqual([
      ["ports", "Run Doctor"],
      ["ca", "Trust now"],
    ]);
  });
});

describe("labels", () => {
  it.each([
    [{ framework: "symfony" }, "Symfony"],
    [{ framework: "other", is_laravel: true }, "Laravel"],
    [{ is_wordpress: true }, "WordPress"],
    [{}, "PHP site"],
  ] as const)("frameworkLabel(%o) is %s", (overrides, expected) => {
    expect(frameworkLabel(site("x", overrides as Partial<SiteEntry>))).toBe(expected);
  });

  it.each([
    [{ framework: "laravel", uses_front_controller: true, web_subpath: "public" }, "Detected · front controller at /public"],
    [{ framework: "laravel", uses_front_controller: false }, "Detected · runs .php files directly"],
    [{ framework: "laravel" }, "Detected"],
    [{ uses_front_controller: true }, "Front controller at /"],
    [{ uses_front_controller: false }, "Runs .php files directly"],
    [{}, ""],
  ] as const)("frameworkDetail(%o) is %s", (overrides, expected) => {
    expect(frameworkDetail(site("x", overrides as Partial<SiteEntry>))).toBe(expected);
  });

  it("counts aliases beyond the primary", () => {
    expect(aliasCount(site("x"))).toBe(0);
    expect(aliasCount(site("x", { domains: ["x.test"] }))).toBe(0);
    expect(aliasCount(site("x", { domains: ["x.test", "*.x.test"] }))).toBe(1);
    const proxy: ProxyEntry = {
      name: "p",
      target: "http://localhost:1",
      secure: false,
      domains: ["p.test", "*.p.test", "docker.test"],
    };
    expect(aliasCount(proxy)).toBe(2);
  });

  it.each([
    [false, true, "Served over plain HTTP"],
    [true, true, "Certificate issued by Yerd local CA · trusted"],
    [true, false, "Certificate issued by Yerd local CA · not trusted"],
    [true, null, "Certificate issued by Yerd local CA · trust unknown"],
    [true, undefined, "Certificate issued by Yerd local CA · trust unknown"],
  ] as const)("httpsDetail(secure %s, trusted %s) is %s", (secure, trusted, expected) => {
    expect(httpsDetail(secure, trusted)).toBe(expected);
  });

  it("derives the serving pill", () => {
    expect(servingState(site("x"), report())).toEqual({ tone: "ok", label: "Serving" });
    expect(servingState(site("x", { apex_shadowed_by: "y" }), report())).toEqual({
      tone: "warn",
      label: "Served by y",
    });
    expect(
      servingState(site("x"), report({ web_unbound: { http: 8080, https: 8443 } })),
    ).toEqual({ tone: "bad", label: "Not serving" });
  });

  it("describes the source", () => {
    expect(sourceDetail(site("x"), "/home/a/Herd", "/home/a")).toBe(
      "Parked in ~/Herd, served automatically",
    );
    expect(sourceDetail(site("x"), null, null)).toBe("Parked, served automatically");
    expect(
      sourceDetail(site("x", { kind: "linked", document_root: "/home/a/Code/x" }), null, "/home/a"),
    ).toBe("Linked from ~/Code/x");
  });

  it("appends the web sub-path to the served path", () => {
    expect(servedPath(site("x"))).toBe("/home/a/Herd/x");
    expect(servedPath(site("x", { web_subpath: "public" }))).toBe("/home/a/Herd/x/public");
  });
});

describe("urlParts", () => {
  it.each([
    ["https://blog.test", { text: "https://blog.test", port: null, path: "" }],
    ["https://blog.test:8443", { text: "https://blog.test", port: "8443", path: "" }],
    [
      "http://localhost:8080/~blog.test",
      { text: "http://localhost", port: "8080", path: "/~blog.test" },
    ],
    ["http://localhost/~blog.test", { text: "http://localhost", port: null, path: "/~blog.test" }],
    ["not a url", { text: "not a url", port: null, path: "" }],
  ])("splits %s", (url, expected) => {
    expect(urlParts(url)).toEqual(expected);
  });
});

describe("tildify", () => {
  it.each([
    ["/Users/a/Herd/x", "/Users/a", "~/Herd/x"],
    ["/Users/a", "/Users/a", "~"],
    ["/Users/a/x", "/Users/a/", "~/x"],
    ["/Users/a2/x", "/Users/a", "/Users/a2/x"],
    ["/Users/a/x", null, "/Users/a/x"],
  ])("tildify(%s, %s) is %s", (path, home, expected) => {
    expect(tildify(path, home)).toBe(expected);
  });
});

describe("untildify", () => {
  it.each([
    ["~/Herd/x", "/Users/a", "/Users/a/Herd/x"],
    ["~", "/Users/a/", "/Users/a"],
    ["~other/x", "/Users/a", "~other/x"],
    ["/srv/x", "/Users/a", "/srv/x"],
    ["~/x", null, "~/x"],
  ])("untildify(%s, %s) is %s", (path, home, expected) => {
    expect(untildify(path, home)).toBe(expected);
  });
});

describe("normalizeDomain", () => {
  it.each([
    ["api.blog.test", "api.blog.test"],
    ["API.Blog.TEST", "api.blog.test"],
    ["api.blog.test.", "api.blog.test"],
    [" *.Blog.test. ", "*.blog.test"],
  ])("normalizes %s to %s", (input, expected) => {
    expect(normalizeDomain(input)).toBe(expected);
  });
});

describe("siteNameTaken", () => {
  it.each<[string, string[], boolean]>([
    ["blog", ["blog", "shop"], true],
    ["Blog", ["blog"], true],
    ["blog", ["BLOG"], true],
    [" blog ", ["blog"], true],
    ["blog2", ["blog"], false],
    ["", ["blog"], false],
    ["blog", [], false],
  ])("%j against %j is %s", (name, existing, expected) => {
    expect(siteNameTaken(name, existing)).toBe(expected);
  });
});
