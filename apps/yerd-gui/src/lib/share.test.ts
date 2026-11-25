import { describe, expect, it } from "vitest";

import {
  siteShare,
  canStartNamed,
  checkProviderStart,
  CLEANUP_UNKNOWN_MESSAGE,
  cleanupFailedMessage,
  cloudflaredLine,
  filterShares,
  hostPlaceholder,
  isShareHostname,
  kindDetail,
  kindLabel,
  loginUrlFrom,
  namedWarningsFor,
  ngrokDomainHolder,
  removeTunnelCopy,
  type ShareItem,
  shareItems,
  siblingShares,
  stateDotClass,
  stateLabel,
  statusDetail,
  subtitle,
  suggestedHost,
  tunnelChip,
  unsharedProviders,
  unsupportedProviderMessage,
} from "./share";
import type { CloudflaredStatus, TunnelInfo, TunnelProviderStatus } from "@/ipc/types";

function quick(site: string, extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site, kind: "quick", state: "running", url: `https://${site}-x.trycloudflare.com`, ...extra };
}

function named(state: TunnelInfo["state"], extra: Partial<TunnelInfo> = {}): TunnelInfo {
  return { site: "@named", kind: "named", state, ...extra };
}

function item(overrides: Partial<ShareItem> = {}): ShareItem {
  return {
    key: "cloudflare:quick:app",
    provider: "cloudflare",
    kind: "quick",
    site: "app",
    staticDomain: false,
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

describe("shareItems", () => {
  it("orders by site, then provider, then quick before domain", () => {
    const items = shareItems(
      [
        quick("zed", { provider: "zrok", url: "https://z.share.zrok.io" }),
        quick("app", { provider: "ngrok", url: "https://a.ngrok-free.app" }),
        quick("app"),
        named("running"),
      ],
      [{ site: "app", hostname: "app.dev" }],
    );
    expect(items.map((i) => i.key)).toEqual([
      "cloudflare:quick:app",
      "cloudflare:domain:app",
      "ngrok:quick:app",
      "zrok:quick:zed",
    ]);
    expect(shareItems([], [])).toEqual([]);
  });

  it("builds per-site items from the tunnel and never lists the named entry", () => {
    const items = shareItems(
      [
        quick("app", { protected: true, last_error: "edge", warnings: ["vite"] }),
        quick("shop", { provider: "ngrok", static_domain: true, url: "https://demo.ngrok-free.app" }),
        named("running"),
      ],
      [],
    );
    expect(items).toEqual([
      {
        key: "cloudflare:quick:app",
        provider: "cloudflare",
        kind: "quick",
        site: "app",
        staticDomain: false,
        url: "https://app-x.trycloudflare.com",
        host: "app-x.trycloudflare.com",
        state: "running",
        protected: true,
        lastError: "edge",
        warnings: ["vite"],
      },
      expect.objectContaining({
        key: "ngrok:quick:shop",
        provider: "ngrok",
        staticDomain: true,
        host: "demo.ngrok-free.app",
      }),
    ]);
    const [waiting] = shareItems([quick("app", { url: undefined, state: "connecting" })], []);
    expect(waiting).toMatchObject({ url: null, host: null, state: "connecting" });
  });

  it("builds domain items with the named entry's state", () => {
    const sites = [
      { site: "zed", hostname: "zed.dev" },
      { site: "blog", hostname: "blog.dev", protected: true },
    ];
    const live = shareItems([named("failed", { last_error: "boom", warnings: ["blog: pinned", "zed: other"] })], sites);
    expect(live.map((i) => i.site)).toEqual(["blog", "zed"]);
    expect(live[0]).toMatchObject({
      provider: "cloudflare",
      kind: "domain",
      url: "https://blog.dev",
      host: "blog.dev",
      state: "failed",
      protected: true,
      lastError: "boom",
      warnings: ["pinned"],
    });
    const [offline] = shareItems([], sites);
    expect(offline).toMatchObject({ state: "offline", lastError: null });
  });

  it.each([
    ["all", ["cloudflare:quick:app", "ngrok:quick:app", "zrok:quick:b"]],
    ["ngrok", ["ngrok:quick:app"]],
    ["zrok", ["zrok:quick:b"]],
    ["cloudflare", ["cloudflare:quick:app"]],
  ] as const)("filters by %s", (filter, keys) => {
    const items = shareItems(
      [quick("app"), quick("app", { provider: "ngrok" }), quick("b", { provider: "zrok" })],
      [],
    );
    expect(filterShares(items, filter).map((i) => i.key)).toEqual(keys);
  });
});

describe("siteShare", () => {
  const quick = (site: string, provider?: "ngrok" | "zrok"): TunnelInfo => ({
    site,
    kind: "quick",
    state: "running",
    url: `https://${site}-${provider ?? "cf"}.example`,
    ...(provider ? { provider } : {}),
  });
  const tunnels = [quick("app"), quick("app", "ngrok"), quick("blog", "ngrok")];

  it.each([
    ["ngrok", "https://app-ngrok.example"],
    ["cloudflare", "https://app-cf.example"],
    [undefined, "https://app-cf.example"],
    ["zrok", null],
  ] as const)("picks the %s share of the site", (provider, url) => {
    expect(siteShare(tunnels, "app", provider)?.url ?? null).toBe(url);
  });
});

describe("checkProviderStart", () => {
  const cf = quick("app");
  const ngrok = quick("app", { provider: "ngrok" });
  const other = quick("blog");

  const zrokBlog = quick("blog", { provider: "zrok" });
  const shop = quick("shop");

  it.each([
    ["cloudflare asked", "cloudflare", [], [cf], false, null, false],
    ["no provider asked", undefined, [], [cf], false, null, false],
    ["the asked share is present", "ngrok", [], [cf, ngrok], false, null, false],
    ["the share is present without a prior list", "ngrok", null, [ngrok], false, null, false],
    ["another site shares through the provider", "zrok", [], [zrokBlog], false, null, false],
    ["an old daemon started a new Cloudflare share", "ngrok", [other], [other, cf], true, "app", false],
    ["the Cloudflare share already existed", "ngrok", [cf], [cf], true, null, false],
    ["the prior list is unknown", "ngrok", null, [cf], true, null, true],
    ["no Cloudflare share came back either", "zrok", [other], [other], true, null, false],
    ["two new Cloudflare shares are ambiguous", "ngrok", [], [cf, shop], true, null, true],
  ] as const)("%s", (_label, provider, before, after, unsupported, stopCloudflare, cleanupUnknown) => {
    expect(checkProviderStart(provider, before, after)).toEqual({ unsupported, stopCloudflare, cleanupUnknown });
  });

  it("points at the Share page when the stray share is unknown", () => {
    expect(CLEANUP_UNKNOWN_MESSAGE).toContain("check the Share page");
  });

  it("points at the Share page when cleanup fails", () => {
    expect(cleanupFailedMessage("app")).toBe(
      "app may still be public through its Cloudflare quick link - stop it on the Share page",
    );
  });

  it("names the provider in the error", () => {
    expect(unsupportedProviderMessage("ngrok")).toBe(
      "The running Yerd daemon doesn't support ngrok sharing yet - restart Yerd and try again",
    );
  });
});

describe("provider-aware share helpers", () => {
  it.each([
    [item(), "Quick link", "Temporary trycloudflare.com URL"],
    [item({ kind: "domain" }), "Your domain", "Stable hostname, survives restarts"],
    [item({ provider: "ngrok", staticDomain: true }), "Reserved domain", "Your static ngrok domain"],
    [item({ provider: "ngrok" }), "Account URL", "The address ngrok assigns to your account"],
    [item({ provider: "zrok" }), "Public share", "New zrok URL each start"],
  ])("labels %#", (share, label, detail) => {
    expect(kindLabel(share)).toBe(label);
    expect(kindDetail(share)).toBe(detail);
  });

  it("lists a site's siblings without the share itself", () => {
    const a = item({ key: "cloudflare:quick:app" });
    const b = item({ key: "ngrok:quick:app", provider: "ngrok" });
    const c = item({ key: "zrok:quick:other", provider: "zrok", site: "other" });
    expect(siblingShares([a, b, c], b)).toEqual([a]);
    expect(siblingShares([c], c)).toEqual([]);
  });

  it("offers the providers a site isn't shared through, with readiness", () => {
    const providers: TunnelProviderStatus[] = [
      { provider: "cloudflare", readiness: "ready", configured: false },
      { provider: "ngrok", readiness: "ready", configured: true },
      { provider: "zrok", readiness: "needs_setup", configured: false },
    ];
    const items = [
      item({ key: "ngrok:quick:app", provider: "ngrok" }),
      item({ key: "cloudflare:domain:app", kind: "domain" }),
    ];
    expect(unsharedProviders(items, "app", providers)).toEqual([
      { provider: "cloudflare", ready: true },
      { provider: "zrok", ready: false },
    ]);
    expect(unsharedProviders([item()], "app", [])).toEqual([
      { provider: "ngrok", ready: false },
      { provider: "zrok", ready: false },
    ]);
  });

  it.each([
    ["demo.ngrok-free.app", "shop"],
    ["Demo.Ngrok-Free.app", "shop"],
    ["other.ngrok-free.app", null],
    [null, null],
  ])("finds who holds the static domain %s", (domain, holder) => {
    const items = [
      item({ provider: "ngrok", site: "shop", url: "https://demo.ngrok-free.app" }),
      item({ site: "app", url: "https://other.ngrok-free.app" }),
    ];
    expect(ngrokDomainHolder(items, domain)).toBe(holder);
  });

  it.each([
    [item(), "Sharing app.test over HTTPS · anyone with the link can reach it"],
    [item({ kind: "domain" }), "Exposed through tunnel yerd · app.test over HTTPS"],
    [
      item({ provider: "ngrok", staticDomain: true }),
      "Sharing app.test over HTTPS through ngrok · reserved domain",
    ],
    [item({ provider: "zrok" }), "Sharing app.test over HTTPS through zrok · public share"],
  ])("subtitle %#", (share, text) => {
    expect(subtitle(share, "app.test", "HTTPS", "yerd")).toBe(text);
  });

  it.each([
    [item({ provider: "ngrok" }), { provider: "ngrok", readiness: "ready", configured: true, version: "3.18.4", source: "system" }, "ngrok 3.18.4 · Found on PATH"],
    [item({ provider: "zrok" }), { provider: "zrok", readiness: "ready", configured: true, source: "managed" }, "zrok · Yerd's copy"],
    [item({ provider: "zrok" }), null, "zrok"],
    [item({ provider: "ngrok", lastError: "ngrok exited" }), null, "ngrok exited"],
  ] as const)("statusDetail for ngrok and zrok %#", (share, status, detail) => {
    expect(statusDetail(share, managed, true, status)).toBe(detail);
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
