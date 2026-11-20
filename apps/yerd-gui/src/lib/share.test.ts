import { describe, expect, it } from "vitest";

import {
  canStartNamed,
  cloudflaredLine,
  hostPlaceholder,
  isShareHostname,
  loginUrlFrom,
  namedWarningsFor,
  removeTunnelCopy,
  type ShareItem,
  shareSections,
  stateDotClass,
  stateLabel,
  statusDetail,
  suggestedHost,
  tunnelChip,
} from "./share";
import type { CloudflaredStatus, TunnelInfo } from "@/ipc/types";

function quick(site: string, extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site, kind: "quick", state: "running", url: `https://${site}-x.trycloudflare.com`, ...extra };
}

function named(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "@named", kind: "named", state, ...extra };
}

function item(overrides: Partial<ShareItem> = {}): ShareItem {
  return {
    key: "quick:app",
    kind: "quick",
    site: "app",
    url: null,
    host: null,
    state: "running",
    protected: false,
    lastError: null,
    warnings: [],
    ...overrides,
  };
}

const managed: CloudflaredStatus = { installed: true, version: "2026.6.1", source: "managed", logged_in: true };

describe("shareSections", () => {
  it("groups quick links before the domain shares and omits empty groups", () => {
    expect(shareSections([quick("app")], [], null).map((s) => s.kind)).toEqual(["quick"]);
    expect(shareSections([], [{ site: "blog", hostname: "blog.dev" }], "dev").map((s) => s.kind)).toEqual([
      "domain",
    ]);
    expect(shareSections([], [], "dev")).toEqual([]);
    const both = shareSections([quick("app")], [{ site: "app", hostname: "app.dev" }], "dev");
    expect(both.map((s) => s.items[0]?.key)).toEqual(["quick:app", "domain:app"]);
  });

  it.each([
    ["forjed.io", "Your domain · forjed.io"],
    [null, "Your domain"],
  ])("labels the domain group for zone %s", (zone, label) => {
    const [section] = shareSections([], [{ site: "a", hostname: "a.x" }], zone);
    expect(section?.label).toBe(label);
  });

  it("builds quick items from the tunnel and never lists the named entry", () => {
    const [section] = shareSections(
      [quick("app", { protected: true, last_error: "edge", warnings: ["vite"] }), named("running")],
      [],
      null,
    );
    expect(section?.items).toEqual([
      {
        key: "quick:app",
        kind: "quick",
        site: "app",
        url: "https://app-x.trycloudflare.com",
        host: "app-x.trycloudflare.com",
        state: "running",
        protected: true,
        lastError: "edge",
        warnings: ["vite"],
      },
    ]);
    const [waiting] = shareSections([quick("app", { url: undefined, state: "connecting" })], [], null);
    expect(waiting?.items[0]).toMatchObject({ url: null, host: null, state: "connecting" });
  });

  it("builds domain items by site name with the named entry's state", () => {
    const sites = [
      { site: "zed", hostname: "zed.dev" },
      { site: "blog", hostname: "blog.dev", protected: true },
    ];
    const [live] = shareSections(
      [named("failed", { last_error: "boom", warnings: ["blog: pinned", "zed: other"] })],
      sites,
      "dev",
    );
    expect(live?.items.map((i) => i.site)).toEqual(["blog", "zed"]);
    expect(live?.items[0]).toMatchObject({
      url: "https://blog.dev",
      host: "blog.dev",
      state: "failed",
      protected: true,
      lastError: "boom",
      warnings: ["pinned"],
    });
    const [offline] = shareSections([], sites, "dev");
    expect(offline?.items[0]).toMatchObject({ state: "offline", lastError: null });
  });
});

describe("named tunnel helpers", () => {
  it("strips the site prefix and drops other sites' warnings", () => {
    const entry = named("running", { warnings: ["app: a", "blog: b", "app: c"] });
    expect(namedWarningsFor(entry, "app")).toEqual(["a", "c"]);
    expect(namedWarningsFor(null, "app")).toEqual([]);
  });

  it.each([
    [null, 1, true],
    [named("failed"), 1, true],
    [named("running"), 1, false],
    [named("reconnecting"), 1, false],
    [null, 0, false],
  ] as const)("canStartNamed(%o, %i) is %s", (entry, count, expected) => {
    expect(canStartNamed(entry, count)).toBe(expected);
  });

  it.each([
    [named("running"), 2, "Live · 2 sites"],
    [named("running"), 1, "Live · 1 site"],
    [named("connecting"), 2, "Connecting"],
    [named("failed"), 2, "Failed, retrying"],
    [null, 2, "Offline"],
  ] as const)("tunnelChip(%o, %i) is %s", (entry, count, chip) => {
    expect(tunnelChip(entry, count)).toBe(chip);
  });
});

describe("labels", () => {
  it.each([
    ["running", "Running", "bg-success"],
    ["connecting", "Connecting", "bg-warning"],
    ["reconnecting", "Reconnecting", "bg-warning"],
    ["failed", "Failed, retrying", "bg-destructive"],
    ["offline", "Offline", "bg-faint"],
  ] as const)("%s reads %s with a %s dot", (state, label, dot) => {
    expect(stateLabel(state)).toBe(label);
    expect(stateDotClass(state)).toBe(dot);
  });

  it.each([
    ["forjed.io", "app.forjed.io"],
    [null, ""],
  ])("suggests a hostname for zone %s", (zone, host) => {
    expect(suggestedHost("app", zone)).toBe(host);
  });

  it.each([
    ["forjed.io", "site.forjed.io"],
    [null, "site.example.com"],
  ])("placeholds the hostname for zone %s", (zone, placeholder) => {
    expect(hostPlaceholder(zone)).toBe(placeholder);
  });

  it.each([
    ["app.forjed.io", "forjed.io", true],
    ["  App.Forjed.io. ", "forjed.io", true],
    ["forjed.io", "forjed.io", true],
    ["a.b.forjed.io", "forjed.io", true],
    ["app.example.com", "forjed.io", false],
    ["appforjed.io", "forjed.io", false],
    ["*.forjed.io", "forjed.io", false],
    ["app..forjed.io", "forjed.io", false],
    ["app_1.forjed.io", "forjed.io", false],
    ["", "forjed.io", false],
    ["app.example.com", null, false],
  ])("isShareHostname(%j, %j)", (host, zone, ok) => {
    expect(isShareHostname(host, zone)).toBe(ok);
  });

  it.each([
    [managed, "cloudflared 2026.6.1 · Yerd's copy"],
    [{ ...managed, source: "system" as const }, "cloudflared 2026.6.1 · on PATH"],
    [{ ...managed, version: undefined }, "cloudflared · Yerd's copy"],
  ])("describes the cloudflared in use", (status, line) => {
    expect(cloudflaredLine(status)).toBe(line);
  });

  it.each([
    [item({ lastError: "edge down", state: "failed" }), "edge down"],
    [item({ kind: "domain", state: "failed" }), "cloudflared exited. Restart to retry."],
    [item({ kind: "domain", state: "offline" }), "The tunnel isn't running. Start it to go live."],
    [item({ kind: "quick", state: "running" }), "cloudflared 2026.6.1 · Yerd's copy"],
  ])("statusDetail prefers the error, then the down tunnel hint", (share, detail) => {
    expect(statusDetail(share, managed)).toBe(detail);
  });

  it("points a down domain share at Setup when logged out", () => {
    expect(statusDetail(item({ kind: "domain", state: "failed" }), managed, false)).toBe(
      "Connect your Cloudflare account in Setup to start it.",
    );
  });
});

describe("loginUrlFrom", () => {
  it.each([
    [["Please open", "https://dash.cloudflare.com/argotunnel?aud=1 now"], "https://dash.cloudflare.com/argotunnel?aud=1"],
    [["Waiting for login…"], null],
    [
      ["https://dash.cloudflare.com/argotunnel?a", "https://dash.cloudflare.com/argotunnel?b"],
      "https://dash.cloudflare.com/argotunnel?a",
    ],
  ])("captures the first auth URL from %o", (lines, url) => {
    expect(loginUrlFrom(lines)).toBe(url);
  });
});

describe("removeTunnelCopy", () => {
  it.each([
    [[], "This deletes the tunnel from your Cloudflare account. DNS records created for it may need removing in the Cloudflare dashboard."],
    [["a"], "takes a offline."],
    [["a", "b"], "takes a and b offline."],
    [["a", "b", "c"], "takes a, b and c offline."],
  ])("names the sites %o", (sites, text) => {
    expect(removeTunnelCopy(sites)).toContain(text);
  });
});
