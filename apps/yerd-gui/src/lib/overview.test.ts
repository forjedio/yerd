import { describe, expect, it } from "vitest";

import type { Contributor, ReleaseNotesEntry, ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";

import {
  cappedList,
  contributorTiles,
  formatReleaseDate,
  healthRows,
  metaSegments,
  nextStep,
  pinnedView,
  portsReady,
  recentlyAdded,
  releaseList,
  releaseStats,
  releaseSummary,
  serviceSummary,
  setupPending,
  setupSteps,
  siteHeadline,
  statusKind,
} from "./overview";

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    daemon_pid: 55921,
    uptime_secs: 660,
    daemon_version: "2.1.0",
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    ca: { trusted_system: true },
    resolver_installed: true,
    php: [{ version: "8.4" }],
    sites: { parked: 3, linked: 1, secured: 0 },
    ...overrides,
  } as unknown as StatusReport;
}

function service(state: string, installed = true): ServiceStatus {
  return { state, installed_versions: installed ? ["1.0"] : [] } as unknown as ServiceStatus;
}

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "blog",
    document_root: "/Users/me/Herd/blog",
    php: "8.4",
    secure: false,
    kind: "parked",
    ...overrides,
  } as SiteEntry;
}

const FELL_BACK = { requested: 80, bound: 8080, fell_back: true };

describe("portsReady", () => {
  it.each([
    ["no fallback", {}, true],
    ["http fell back from 80 without a redirect", { http: FELL_BACK }, false],
    ["https fell back from 443 without a redirect", { https: { requested: 443, bound: 8443, fell_back: true } }, false],
    ["fell back but the redirect carries it", { http: FELL_BACK, port_redirect: true }, true],
    ["fell back from an unprivileged port", { http: { requested: 8000, bound: 8001, fell_back: true } }, true],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(portsReady(report(overrides as Partial<StatusReport>))).toBe(expected);
  });
});

describe("statusKind and siteHeadline", () => {
  it.each([
    [0, {}, "ready", "No sites yet", false],
    [1, {}, "serving", "1 site", true],
    [62, {}, "serving", "62 sites", true],
    [62, { web_unbound: { http: 8080, https: 8443 } }, "not-serving", "0 of 62 sites", true],
    [0, { web_unbound: { http: 8080, https: 8443 } }, "not-serving", "0 of 0 sites", true],
  ] as const)("%i sites with %o", (count, overrides, kind, text, showTld) => {
    const k = statusKind(report(overrides as Partial<StatusReport>), count);
    expect(k).toBe(kind);
    expect(siteHeadline(k, count)).toEqual({ text, showTld });
  });
});

describe("metaSegments", () => {
  it.each([
    ["no services", {}, ["yerdd 2.1.0", "up 11m 0s", "pid 55921"]],
    [
      "some services running",
      { services: [service("running"), service("stopped"), service("running")] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "2 of 3 services running"],
    ],
    [
      "one service running",
      { services: [service("running")] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "1 of 1 service running"],
    ],
    [
      "services but none running",
      { services: [service("stopped")] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "0 of 1 service running"],
    ],
    [
      "only uninstalled engines",
      { services: [service("stopped", false)] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921"],
    ],
    ["no PHP installed", { php: [] }, ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "no PHP installed"]],
    ["unknown daemon version", { daemon_version: "" }, ["yerdd unknown", "up 11m 0s", "pid 55921"]],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(metaSegments(report(overrides as unknown as Partial<StatusReport>))).toEqual(expected);
  });
});

describe("healthRows", () => {
  it("gives each healthy check its own ok chip", () => {
    expect(healthRows(report())).toEqual([
      { key: "trust", tone: "ok", text: "Local CA trusted" },
      { key: "resolver", tone: "ok", text: ".test resolver installed" },
    ]);
  });

  it("gives each failing check its own warn row and drops the ok row", () => {
    const rows = healthRows(
      report({ ca: { trusted_system: false } as StatusReport["ca"], resolver_installed: null }),
    );
    expect(rows.map((r) => [r.key, r.tone, r.text, r.link?.to])).toEqual([
      ["trust", "warn", "Local CA isn't trusted", "/doctor"],
      ["resolver", "warn", ".test resolver status is unknown", "/doctor"],
    ]);
  });

  it("keeps the ok row beside a single failing check", () => {
    const rows = healthRows(
      report({ ca: { trusted_system: null } as StatusReport["ca"], resolver_installed: false }),
    );
    expect(rows.map((r) => r.text)).toEqual([
      "Local CA trust is unknown",
      ".test resolver isn't installed",
    ]);
    const half = healthRows(report({ resolver_installed: false }));
    expect(half.map((r) => r.tone)).toEqual(["ok", "warn"]);
    expect(half[0]?.text).toBe("Local CA trusted");
  });

  it("names a foreign listener when one holds the ports", () => {
    const rows = healthRows(report({ http: FELL_BACK, foreign_web_listener: true }));
    expect(rows[2]).toEqual({
      key: "ports",
      tone: "warn",
      text: "80/443 busy, serving on :8080 / :443",
      link: { label: "Fix", to: "/doctor" },
    });
  });

  it("says the ports aren't forwarded when nothing foreign holds them", () => {
    const rows = healthRows(
      report({
        http: FELL_BACK,
        https: { requested: 443, bound: 8443, fell_back: true },
        foreign_web_listener: false,
      }),
    );
    expect(rows[2]?.text).toBe("80/443 not forwarded, serving on :8080 / :8443");
  });

  it("prefers the bind failure over the fallback row and links Settings", () => {
    const rows = healthRows(
      report({ http: FELL_BACK, web_unbound: { http: 8080, https: 8443 } }),
    );
    expect(rows.slice(2)).toEqual([
      {
        key: "ports",
        tone: "bad",
        text: "Couldn't bind web ports 8080/8443 — they're in use",
        link: { label: "Change ports", to: "/general" },
      },
    ]);
  });

  it("flags an unbound DNS port instead of a green resolver row", () => {
    const rows = healthRows(report({ dns_unbound: 1053 }));
    expect(rows).toEqual([
      { key: "trust", tone: "ok", text: "Local CA trusted" },
      {
        key: "dns",
        tone: "bad",
        text: "Couldn't bind DNS port 1053 — .test names won't resolve",
        link: { label: "Change port", to: "/general" },
      },
    ]);
  });

  it("uses the configured tld", () => {
    expect(healthRows(report({ tld: "dev" }))[1]?.text).toBe(".dev resolver installed");
  });
});

describe("pinnedView", () => {
  const groups = { order: ["Acme", "Side", "Empty"], members: { b: "Acme", a: "acme", c: "Acme", d: "Side", e: "Gone" } };

  it.each([
    [
      "groups starred sites in group order, ungrouped below",
      [
        site({ name: "c", favourite: true }),
        site({ name: "a", favourite: true }),
        site({ name: "b" }),
        site({ name: "d" }),
        site({ name: "z", favourite: true }),
        site({ name: "e", favourite: true }),
      ],
      [["Acme", ["a", "c"], 3]],
      ["e", "z"],
    ],
    ["drops a group with no stars", [site({ name: "d" }), site({ name: "b" })], [], []],
    [
      "treats a site named like a prototype member as ungrouped",
      [site({ name: "constructor", favourite: true }), site({ name: "toString", favourite: true })],
      [],
      ["constructor", "toString"],
    ],
    [
      "lists a group per starred member",
      [site({ name: "d", favourite: true }), site({ name: "b", favourite: true })],
      [
        ["Acme", ["b"], 1],
        ["Side", ["d"], 1],
      ],
      [],
    ],
  ] as const)("%s", (_name, sites, expectedGroups, ungrouped) => {
    const view = pinnedView(sites as unknown as SiteEntry[], groups);
    expect(view.groups.map((g) => [g.name, g.starred.map((s) => s.name), g.total])).toEqual(expectedGroups);
    expect(view.ungrouped.map((s) => s.name)).toEqual(ungrouped);
  });
});

describe("recentlyAdded", () => {
  it.each([
    [
      "newest first, undated left out",
      [site({ name: "a", added_at: 10 }), site({ name: "b" }), site({ name: "c", added_at: 30 })],
      7,
      ["c", "a"],
    ],
    [
      "ties broken by name",
      [site({ name: "z", added_at: 5 }), site({ name: "m", added_at: 5 })],
      7,
      ["m", "z"],
    ],
    [
      "capped at the limit",
      [1, 2, 3, 4].map((n) => site({ name: `s${n}`, added_at: n })),
      2,
      ["s4", "s3"],
    ],
    ["nothing dated", [site({ name: "a" })], 7, []],
  ] as const)("%s", (_name, sites, limit, expected) => {
    expect(recentlyAdded(sites as unknown as SiteEntry[], limit).map((s) => s.name)).toEqual(expected);
  });
});

describe("serviceSummary", () => {
  it.each([
    ["counts installed rows only", [service("running"), service("stopped"), service("stopped", false)], 2, 1],
    ["per-site instances count as installed", [{ state: "running", installed_versions: [], site: "blog" }], 1, 1],
    ["nothing installed", [service("stopped", false)], 0, 0],
  ] as const)("%s", (_name, services, rows, running) => {
    const out = serviceSummary(services as unknown as ServiceStatus[]);
    expect(out.rows).toHaveLength(rows);
    expect(out.running).toBe(running);
  });
});

describe("contributorTiles", () => {
  const people = (n: number) =>
    Array.from({ length: n }, (_, i) => ({ login: `u${i}`, avatar_url: "", commits: 1 }) as Contributor);

  it("shows the first tiles and counts the rest", () => {
    const out = contributorTiles(people(12), 12, 5);
    expect(out.shown.map((c) => c.login)).toEqual(["u0", "u1", "u2", "u3", "u4"]);
    expect(out.more).toBe(7);
  });

  it("has no more tile when everyone fits", () => {
    expect(contributorTiles(people(2), 2, 3)).toEqual({ shown: people(2), more: 0 });
  });
});

function entry(overrides: Partial<ReleaseNotesEntry> = {}): ReleaseNotesEntry {
  return {
    version: "2.1.0",
    tag: "v2.1.0",
    published_at: "2026-08-15T10:00:00Z",
    prerelease: false,
    summary: "The summary",
    features: [],
    fixes: [],
    others: [],
    ...overrides,
  };
}

const items = (prefix: string, n: number, author?: string) =>
  Array.from({ length: n }, (_, i) => ({ title: `${prefix} ${i}`, author }));

describe("releaseSummary", () => {
  it.each([
    ["several features", { features: items("F", 3), fixes: items("X", 2) }, { lead: "F 0", more: "and 2 more features" }],
    ["two features", { features: items("F", 2) }, { lead: "F 0", more: "and 1 more feature" }],
    ["a single feature", { features: items("F", 1) }, { lead: "F 0", more: null }],
    ["fixes only", { fixes: items("X", 4) }, { lead: "X 0", more: "and 3 more fixes" }],
    ["one fix", { fixes: items("X", 1) }, { lead: "X 0", more: null }],
    ["others only", { others: items("O", 2) }, { lead: "The summary", more: null }],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(releaseSummary(entry(overrides as Partial<ReleaseNotesEntry>))).toEqual(expected);
  });
});

describe("releaseStats", () => {
  it.each([
    [
      "counts items and distinct authors",
      {
        features: [...items("F", 2, "ann"), ...items("G", 1, "bob")],
        fixes: items("X", 1, "ann"),
        others: items("O", 1),
      },
      ["5 pull requests", "3 features", "1 fix", "2 contributors"],
    ],
    ["an empty release", {}, ["0 pull requests", "0 features", "0 fixes", "0 contributors"]],
    ["one of each", { features: items("F", 1, "ann") }, ["1 pull request", "1 feature", "0 fixes", "1 contributor"]],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(releaseStats(entry(overrides as unknown as Partial<ReleaseNotesEntry>))).toEqual(expected);
  });
});

describe("cappedList", () => {
  it.each([
    ["below the cap", 3, 4, 3, 0],
    ["at the cap", 4, 4, 4, 0],
    ["above the cap", 15, 4, 4, 11],
    ["empty", 0, 6, 0, 0],
  ] as const)("%s", (_name, n, cap, shown, more) => {
    const out = cappedList(items("I", n), cap);
    expect(out.shown).toHaveLength(shown);
    expect(out.more).toBe(more);
  });
});

describe("releaseList", () => {
  it("puts the current release first", () => {
    const list = releaseList({ current: entry({ tag: "v2" }), earlier: [entry({ tag: "v1" })] });
    expect(list.map((e) => e.tag)).toEqual(["v2", "v1"]);
  });

  it("skips a missing current release", () => {
    expect(releaseList({ earlier: [entry({ tag: "v1" })] }).map((e) => e.tag)).toEqual(["v1"]);
  });
});

describe("setupSteps and nextStep", () => {
  const empty = { php: [], sites: { parked: 0, linked: 0, secured: 0 } };

  it.each([
    ["a fresh machine with a trusted CA", empty, 0, [true, false, false], "php"],
    ["an untrusted CA", { ...empty, ca: { trusted_system: null } }, 0, [false, false, false], "trust"],
    ["an empty parked root", empty, 1, [true, false, true], "php"],
    ["PHP installed, nothing parked yet", { sites: { parked: 0, linked: 0, secured: 0 } }, 0, [true, true, false], "park"],
    ["a linked site counts as parked", { php: [], sites: { parked: 0, linked: 1, secured: 0 } }, 0, [true, false, true], "php"],
    ["everything done", {}, 0, [true, true, true], null],
  ] as const)("%s", (_name, overrides, roots, done, next) => {
    const steps = setupSteps(report(overrides as unknown as Partial<StatusReport>), roots);
    expect(steps.map((s) => s.key)).toEqual(["trust", "php", "park"]);
    expect(steps.map((s) => s.done)).toEqual(done);
    expect(nextStep(steps)).toBe(next);
  });
});

describe("setupPending", () => {
  const none = { sites: { parked: 0, linked: 0, secured: 0 } };

  it.each([
    ["a fresh machine", { ...none, php: [] }, 0, true],
    ["PHP installed but nothing parked", none, 0, true],
    ["a parked root but no PHP", { ...none, php: [] }, 1, true],
    ["PHP and a parked root", none, 1, false],
    ["PHP and a linked site", { sites: { parked: 0, linked: 1, secured: 0 } }, 0, false],
    ["set up but the CA untrusted", { ca: { trusted_system: false } }, 0, false],
  ] as const)("%s", (_name, overrides, roots, expected) => {
    const steps = setupSteps(report(overrides as unknown as Partial<StatusReport>), roots);
    expect(setupPending(steps)).toBe(expected);
  });
});

describe("formatReleaseDate", () => {
  it.each([
    ["2026-08-15T10:00:00Z", "15 Aug 2026"],
    ["2026-07-25T23:30:00-02:00", "26 Jul 2026"],
    ["not a date", ""],
  ])("%s", (iso, expected) => {
    expect(formatReleaseDate(iso)).toBe(expected);
  });
});
