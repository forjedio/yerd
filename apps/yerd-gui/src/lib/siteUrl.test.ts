import { describe, expect, it } from "vitest";

import { wpAdminLoginUrl, wpAdminUrl } from "./siteUrl";
import type { StatusReport } from "@/ipc/types";

function report(overrides: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    dns_unbound: null,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    port_redirect: false,
    ...overrides,
  } as StatusReport;
}

describe("wpAdminLoginUrl", () => {
  it("appends the token as a query param onto the plain wp-admin URL", () => {
    const site = { name: "blog", secure: true };
    const r = report();
    expect(wpAdminLoginUrl(site, r, "abc123")).toBe(
      `${wpAdminUrl(site, r)}?yerd_login_token=abc123`,
    );
  });

  it("URL-encodes a token containing reserved characters", () => {
    const site = { name: "blog", secure: false };
    const r = report();
    expect(wpAdminLoginUrl(site, r, "a&b=c")).toBe(
      `${wpAdminUrl(site, r)}?yerd_login_token=a%26b%3Dc`,
    );
  });
});
