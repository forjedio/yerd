import { describe, expect, it } from "vitest";

import type { ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";

import {
  favouriteSites,
  formatReleaseDate,
  healthRows,
  metaSegments,
  nextStep,
  portsReady,
  setupPending,
  setupSteps,
  siteHeadline,
  siteSubtitle,
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

function service(state: string): ServiceStatus {
  return { state } as unknown as ServiceStatus;
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
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "2 services running"],
    ],
    [
      "one service running",
      { services: [service("running")] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "1 service running"],
    ],
    [
      "services but none running",
      { services: [service("stopped")] },
      ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "0 services running"],
    ],
    ["no PHP installed", { php: [] }, ["yerdd 2.1.0", "up 11m 0s", "pid 55921", "no PHP installed"]],
    ["unknown daemon version", { daemon_version: "" }, ["yerdd unknown", "up 11m 0s", "pid 55921"]],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(metaSegments(report(overrides as unknown as Partial<StatusReport>))).toEqual(expected);
  });
});

describe("healthRows", () => {
  it("collapses a healthy report into one ok row", () => {
    expect(healthRows(report())).toEqual([
      { key: "ok", tone: "ok", text: "Local CA trusted · .test resolver installed" },
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
    expect(rows[1]).toEqual({
      key: "ports",
      tone: "warn",
      text: "Ports 80/443 are in use — serving on :8080 / :443",
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
    expect(rows[1]?.text).toBe("Ports 80/443 aren't forwarded — serving on :8080 / :8443");
  });

  it("prefers the bind failure over the fallback row and links Settings", () => {
    const rows = healthRows(
      report({ http: FELL_BACK, web_unbound: { http: 8080, https: 8443 } }),
    );
    expect(rows.slice(1)).toEqual([
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
      { key: "ok", tone: "ok", text: "Local CA trusted" },
      {
        key: "dns",
        tone: "bad",
        text: "Couldn't bind DNS port 1053 — .test names won't resolve",
        link: { label: "Change port", to: "/general" },
      },
    ]);
  });

  it("uses the configured tld", () => {
    expect(healthRows(report({ tld: "dev" }))[0]?.text).toBe(
      "Local CA trusted · .dev resolver installed",
    );
  });
});

describe("favouriteSites", () => {
  it("keeps only starred sites, sorted by name", () => {
    const out = favouriteSites([
      site({ name: "zeta", favourite: true }),
      site({ name: "alpha" }),
      site({ name: "beta", favourite: true }),
    ]);
    expect(out.map((s) => s.name)).toEqual(["beta", "zeta"]);
  });
});

describe("siteSubtitle", () => {
  it.each([
    ["a parked site", {}, "/Users/me/Herd/blog"],
    ["a linked site", { kind: "linked" }, "/Users/me/Herd/blog · linked"],
    ["a WordPress site", { framework: "wordpress" }, "/Users/me/Herd/blog · WordPress"],
    ["a linked Laravel site", { kind: "linked", framework: "laravel" }, "/Users/me/Herd/blog · linked · Laravel"],
    ["an unrecognised framework", { framework: "other" }, "/Users/me/Herd/blog"],
  ] as const)("%s", (_name, overrides, expected) => {
    expect(siteSubtitle(site(overrides as Partial<SiteEntry>))).toBe(expected);
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
