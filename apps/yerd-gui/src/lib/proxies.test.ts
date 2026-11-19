import { describe, expect, it } from "vitest";

import {
  addSummary,
  extraDomains,
  hasScheme,
  isValidPrefix,
  isValidProxyName,
  normalizeUpstream,
  proxyChips,
  proxyHost,
  proxyNameFromInput,
  proxySections,
  ruleSite,
  upstreamLabel,
} from "./proxies";
import type { ProxyEntry, ProxyRuleEntry, SiteEntry } from "@/ipc/types";

function proxy(name: string, overrides: Partial<ProxyEntry> = {}): ProxyEntry {
  return { name, target: "http://localhost:3000", secure: false, ...overrides };
}

function rule(site: string, prefix: string): ProxyRuleEntry {
  return { site, prefix, target: "http://127.0.0.1:8081" };
}

function site(name: string, secure = false): SiteEntry {
  return { name, document_root: `/srv/${name}`, php: "8.4", secure, kind: "linked" };
}

describe("upstreams", () => {
  it.each([
    ["http://localhost:9011/", "http://localhost:9011"],
    ["localhost:9011", "http://localhost:9011"],
    ["  127.0.0.1:8081//  ", "http://127.0.0.1:8081"],
    ["https://localhost:8443", "https://localhost:8443"],
    ["HTTPS://x:1/", "HTTPS://x:1"],
    ["http://", "http://"],
    ["", ""],
  ])("normalizeUpstream(%j) is %j", (raw, expected) => {
    expect(normalizeUpstream(raw)).toBe(expected);
  });

  it.each([
    ["http://localhost:3000", "localhost:3000"],
    ["HTTP://localhost:3000", "localhost:3000"],
    ["https://localhost:8443", "https://localhost:8443"],
    ["localhost:3000", "localhost:3000"],
  ])("upstreamLabel(%j) is %j", (target, expected) => {
    expect(upstreamLabel(target)).toBe(expected);
  });

  it.each([
    ["localhost:3000", false],
    ["http://x", true],
    [" HTTPS://x", true],
    ["", false],
  ])("hasScheme(%j) is %s", (raw, expected) => {
    expect(hasScheme(raw)).toBe(expected);
  });
});

describe("names and prefixes", () => {
  it.each([
    ["grafana", true],
    ["api.account", true],
    [" my-app ", true],
    ["api..account", false],
    ["api account", false],
    [".lead", false],
    ["", false],
  ])("isValidProxyName(%j) is %s", (raw, expected) => {
    expect(isValidProxyName(raw)).toBe(expected);
  });

  it.each([
    ["grafana", "grafana"],
    [" grafana.test ", "grafana"],
    ["Grafana.TEST", "Grafana"],
    ["grafana.test.test", "grafana.test"],
    ["grafana.local", "grafana.local"],
    [".test", ""],
  ])("proxyNameFromInput(%j) is %j", (raw, expected) => {
    expect(proxyNameFromInput(raw, "test")).toBe(expected);
  });

  it.each([
    ["/live", true],
    ["  /app", true],
    ["live", false],
    ["", false],
  ])("isValidPrefix(%j) is %s", (raw, expected) => {
    expect(isValidPrefix(raw)).toBe(expected);
  });
});

describe("chips and sections", () => {
  const proxies = [proxy("mydocker"), proxy("api.account"), proxy("grafana")];
  const rules = [rule("vito", "/ws"), rule("codestash", "/live"), rule("codestash", "/app")];

  it("counts every chip and keeps all three at zero", () => {
    expect(proxyChips(proxies, rules)).toEqual([
      { key: "all", label: "All", count: 6 },
      { key: "domains", label: "Domains", count: 3 },
      { key: "paths", label: "Paths", count: 3 },
    ]);
    expect(proxyChips([], []).map((c) => c.count)).toEqual([0, 0, 0]);
  });

  it("sorts proxies by name and rules by site then prefix", () => {
    const sections = proxySections(proxies, rules, "all");
    expect(sections.map((s) => s.label)).toEqual(["Domains", "Paths on sites"]);
    expect(sections[0]?.items.map((i) => i.key)).toEqual([
      "proxy:api.account",
      "proxy:grafana",
      "proxy:mydocker",
    ]);
    expect(sections[1]?.items.map((i) => i.key)).toEqual([
      "rule:codestash/app",
      "rule:codestash/live",
      "rule:vito/ws",
    ]);
  });

  it.each([
    ["domains", ["Domains"]],
    ["paths", ["Paths on sites"]],
  ] as const)("filters to the %s chip", (chip, labels) => {
    expect(proxySections(proxies, rules, chip).map((s) => s.label)).toEqual(labels);
  });

  it("drops empty sections", () => {
    expect(proxySections(proxies, [], "all").map((s) => s.kind)).toEqual(["proxy"]);
    expect(proxySections([], rules, "domains")).toEqual([]);
  });
});

describe("domains and sites", () => {
  it("hosts a proxy at its primary domain", () => {
    expect(proxyHost(proxy("grafana"), "test")).toBe("grafana.test");
    expect(proxyHost(proxy("grafana", { primary_domain: "g.test" }), "test")).toBe("g.test");
  });

  it.each([
    ["a default proxy", proxy("app"), []],
    [
      "a customised proxy whose primary isn't first",
      proxy("app", { primary_domain: "docker.test", domains: ["app.test", "docker.test"] }),
      ["app.test"],
    ],
    [
      "a proxy with a wildcard, exact names first",
      proxy("app", { domains: ["app.test", "*.app.test", "docker.test"] }),
      ["docker.test", "*.app.test"],
    ],
  ])("lists the extra domains of %s", (_label, p, expected) => {
    expect(extraDomains(p, "test")).toEqual(expected);
  });

  it("finds a rule's site", () => {
    const sites = [site("codestash", true), site("vito")];
    expect(ruleSite(rule("codestash", "/live"), sites)?.secure).toBe(true);
    expect(ruleSite(rule("gone", "/x"), sites)).toBeNull();
  });
});

describe("addSummary", () => {
  it.each([
    [{ mode: "domain", name: "grafana", url: "localhost:3000", tld: "test" }, "grafana.test", "localhost:3000"],
    [{ mode: "domain", name: "grafana.test", url: "https://x:1/", tld: "test" }, "grafana.test", "https://x:1"],
    [{ mode: "domain", name: " ", url: "", tld: "test" }, "name.test", "upstream"],
    [
      { mode: "path", site: "codestash", prefix: "/live", url: "127.0.0.1:8081", tld: "test" },
      "codestash.test/live",
      "127.0.0.1:8081",
    ],
    [{ mode: "path", site: "", prefix: "", url: "", tld: "test" }, "site.test/path", "upstream"],
  ] as const)("summarises %o", (input, from, to) => {
    expect(addSummary(input)).toEqual({ from, to });
  });
});
