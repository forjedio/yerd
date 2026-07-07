import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mintWordPressLoginToken = vi.fn();
const openInBrowser = vi.fn();
const openPath = vi.fn();
vi.mock("@/ipc/client", () => ({
  mintWordPressLoginToken: (...args: unknown[]) => mintWordPressLoginToken(...args),
  openInBrowser: (...args: unknown[]) => openInBrowser(...args),
  openPath: (...args: unknown[]) => openPath(...args),
}));

import SiteCard from "./SiteCard.vue";
import type { SiteEntry, StatusReport } from "@/ipc/types";

const DropdownMenuStub = { template: "<div><slot /></div>" };
const DropdownMenuItemStub = {
  template: '<div class="dropdown-item-stub" @click="$emit(\'select\')"><slot /></div>',
};
const dropdownStubs = {
  DropdownMenu: DropdownMenuStub,
  DropdownMenuTrigger: DropdownMenuStub,
  DropdownMenuContent: DropdownMenuStub,
  DropdownMenuItem: DropdownMenuItemStub,
  DropdownMenuSeparator: { template: "<hr />" },
};

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

function mountCard(site: SiteEntry, report: StatusReport | null) {
  return mount(SiteCard, {
    props: { site, report, tld: "test" },
    global: { stubs: dropdownStubs },
  });
}

/** Clicks the always-visible "WP Admin" dropdown item (works regardless of
 *  `wp_auto_login`, unlike the WPA quick-action chip which only renders when
 *  auto-login is on). */
async function clickWpAdminMenuItem(wrapper: ReturnType<typeof mountCard>) {
  const items = wrapper.findAll(".dropdown-item-stub");
  const item = items.find((i) => i.text().includes("WP Admin"));
  if (!item) throw new Error("WP Admin dropdown item not rendered");
  await item.trigger("click");
}

describe("SiteCard openWpAdmin gating", () => {
  beforeEach(() => {
    mintWordPressLoginToken.mockReset();
    openInBrowser.mockReset();
    openPath.mockReset();
  });

  it("skips minting and opens the plain link in unbound mode, even with auto-login on", async () => {
    const site = wpSite({ wp_auto_login: true });
    const wrapper = mountCard(site, null); // no report => isUnbound() is true

    await clickWpAdminMenuItem(wrapper);

    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("http://localhost:8080/~blog.test/wp-admin/");
  });

  it("skips minting and opens the plain link when auto-login is off", async () => {
    const site = wpSite({ wp_auto_login: false });
    const wrapper = mountCard(site, boundReport());

    await clickWpAdminMenuItem(wrapper);

    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("http://blog.test/wp-admin/");
  });

  it("mints a token and opens the pre-authenticated link when bound and auto-login is on", async () => {
    const site = wpSite({ wp_auto_login: true });
    mintWordPressLoginToken.mockResolvedValue("sekrit-token");
    const wrapper = mountCard(site, boundReport());

    await clickWpAdminMenuItem(wrapper);

    expect(mintWordPressLoginToken).toHaveBeenCalledWith("blog");
    expect(openInBrowser).toHaveBeenCalledWith(
      "http://blog.test/wp-admin/?yerd_login_token=sekrit-token",
    );
  });

  it("falls back to the plain link when minting fails", async () => {
    const site = wpSite({ wp_auto_login: true });
    mintWordPressLoginToken.mockRejectedValue(new Error("daemon unreachable"));
    const wrapper = mountCard(site, boundReport());

    await clickWpAdminMenuItem(wrapper);

    expect(mintWordPressLoginToken).toHaveBeenCalledWith("blog");
    expect(openInBrowser).toHaveBeenCalledWith("http://blog.test/wp-admin/");
  });

  it("the WPA quick-action chip drives the same gating", async () => {
    const site = wpSite({ wp_auto_login: true });
    mintWordPressLoginToken.mockResolvedValue("sekrit-token");
    const wrapper = mountCard(site, boundReport());

    const chip = wrapper.findAll("button").find((b) => b.text() === "WPA");
    if (!chip) throw new Error("WPA chip not rendered");
    await chip.trigger("click");

    expect(openInBrowser).toHaveBeenCalledWith(
      "http://blog.test/wp-admin/?yerd_login_token=sekrit-token",
    );
  });
});
