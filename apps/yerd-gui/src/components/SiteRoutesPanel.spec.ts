import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const listRoutes = vi.fn();
const addRouteRule = vi.fn();
const removeRouteRule = vi.fn();
vi.mock("@/ipc/client", () => ({
  listRoutes: (...a: unknown[]) => listRoutes(...a),
  addRouteRule: (...a: unknown[]) => addRouteRule(...a),
  removeRouteRule: (...a: unknown[]) => removeRouteRule(...a),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import SiteRoutesPanel from "./SiteRoutesPanel.vue";
import type { RouteRuleEntry, SiteEntry } from "@/ipc/types";

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "portal",
    document_root: "/srv/portal",
    php: "8.3",
    secure: false,
    kind: "linked",
    ...overrides,
  };
}

const rule = (s: string, prefix: string, target: string): RouteRuleEntry => ({
  site: s,
  prefix,
  target,
});

async function mountPanel(s: SiteEntry = site()) {
  const wrapper = mount(SiteRoutesPanel, { props: { site: s } });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  listRoutes.mockReset().mockResolvedValue([]);
  addRouteRule.mockReset().mockResolvedValue(undefined);
  removeRouteRule.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("SiteRoutesPanel", () => {
  it("lists only this site's rules, sorted by prefix", async () => {
    listRoutes.mockResolvedValue([
      rule("portal", "/api", "api/index.php"),
      rule("other", "/x", "x/index.php"),
      rule("portal", "/admin", "admin/index.php"),
    ]);
    const wrapper = await mountPanel();
    const rows = wrapper.findAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("/admin");
    expect(rows[1].text()).toContain("/api");
    expect(wrapper.text()).not.toContain("/x");
  });

  it("shows an empty state when the site has no rules", async () => {
    const wrapper = await mountPanel();
    expect(wrapper.text()).toContain("No custom routing rules for this site.");
  });

  it("adds a rule, clears the inputs, and refetches", async () => {
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Route prefix"]').setValue("/api");
    await wrapper.find('[aria-label="Route target"]').setValue("api/index.php");
    listRoutes.mockResolvedValue([rule("portal", "/api", "api/index.php")]);

    await wrapper.find("button").trigger("click");
    await flushPromises();

    expect(addRouteRule).toHaveBeenCalledWith("portal", "/api", "api/index.php");
    expect(listRoutes).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalled();
    expect(wrapper.findAll("li")).toHaveLength(1);
  });

  it("blocks a relative prefix without calling the daemon", async () => {
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Route prefix"]').setValue("api");
    await wrapper.find('[aria-label="Route target"]').setValue("api/index.php");

    expect(wrapper.text()).toContain("A prefix must begin with '/'.");
    await wrapper.find("button").trigger("click");
    await flushPromises();
    expect(addRouteRule).not.toHaveBeenCalled();
  });

  it("removes a rule by its prefix", async () => {
    listRoutes.mockResolvedValue([rule("portal", "/api", "api/index.php")]);
    const wrapper = await mountPanel();

    await wrapper.find('[aria-label="Remove route /api"]').trigger("click");
    await flushPromises();

    expect(removeRouteRule).toHaveBeenCalledWith("portal", "/api");
    expect(toastSuccess).toHaveBeenCalled();
  });

  it("surfaces a daemon rejection as an error toast", async () => {
    addRouteRule.mockRejectedValue(new Error("target must be a relative path"));
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Route prefix"]').setValue("/api");
    await wrapper.find('[aria-label="Route target"]').setValue("../escape.php");

    await wrapper.find("button").trigger("click");
    await flushPromises();

    expect(toastError).toHaveBeenCalledWith(
      "Routing change failed",
      "target must be a relative path",
    );
  });

  it("refetches when the sidebar swaps to a different site", async () => {
    const wrapper = await mountPanel();
    expect(listRoutes).toHaveBeenCalledTimes(1);

    await wrapper.setProps({ site: site({ name: "dashboard" }) });
    await flushPromises();

    expect(listRoutes).toHaveBeenCalledTimes(2);
  });
});
