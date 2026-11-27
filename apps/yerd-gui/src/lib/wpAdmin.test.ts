import { beforeEach, describe, expect, it, vi } from "vitest";

const mintWordPressLoginToken = vi.fn();
const openInBrowser = vi.fn();
vi.mock("@/ipc/client", () => ({
  mintWordPressLoginToken: (...args: unknown[]) => mintWordPressLoginToken(...args),
  openInBrowser: (...args: unknown[]) => openInBrowser(...args),
}));

import { openWpAdmin } from "./wpAdmin";
import type { SiteEntry, StatusReport } from "@/ipc/types";

function wpSite(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "blog",
    document_root: "/srv/blog",
    php: "8.3",
    secure: false,
    kind: "linked",
    is_wordpress: true,
    wp_auto_login: false,
    ...overrides,
  };
}

function boundReport(): StatusReport {
  return {
    resolver_installed: true,
    dns_unbound: null,
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
  } as unknown as StatusReport;
}

describe("openWpAdmin gating", () => {
  beforeEach(() => {
    mintWordPressLoginToken.mockReset();
    openInBrowser.mockReset();
  });

  it("skips minting and opens the plain link in unbound mode, even with auto-login on", async () => {
    await openWpAdmin(wpSite({ wp_auto_login: true }), null);
    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("http://localhost:8080/~blog.test/wp-admin/");
  });

  it("mints a token and opens the pre-authenticated link when bound and auto-login is on", async () => {
    mintWordPressLoginToken.mockResolvedValue("sekrit-token");
    await openWpAdmin(wpSite({ wp_auto_login: true }), boundReport());
    expect(mintWordPressLoginToken).toHaveBeenCalledWith("blog");
    expect(openInBrowser).toHaveBeenCalledWith(
      "http://blog.test/wp-admin/?yerd_login_token=sekrit-token",
    );
  });

  it("falls back to the plain link when minting fails", async () => {
    mintWordPressLoginToken.mockRejectedValue(new Error("daemon unreachable"));
    await openWpAdmin(wpSite({ wp_auto_login: true }), boundReport());
    expect(mintWordPressLoginToken).toHaveBeenCalledWith("blog");
    expect(openInBrowser).toHaveBeenCalledWith("http://blog.test/wp-admin/");
  });

  it("opens the plain link without minting when auto-login is off", async () => {
    await openWpAdmin(wpSite({ wp_auto_login: false }), boundReport());
    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("http://blog.test/wp-admin/");
  });
});
