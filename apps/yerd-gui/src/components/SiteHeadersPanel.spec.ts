import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const listResponseHeaders = vi.fn();
const setResponseHeader = vi.fn();
const removeResponseHeader = vi.fn();
vi.mock("@/ipc/client", () => ({
  listResponseHeaders: (...a: unknown[]) => listResponseHeaders(...a),
  setResponseHeader: (...a: unknown[]) => setResponseHeader(...a),
  removeResponseHeader: (...a: unknown[]) => removeResponseHeader(...a),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import SiteHeadersPanel from "./SiteHeadersPanel.vue";
import type { ResponseHeaderEntry, SiteEntry } from "@/ipc/types";

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "api",
    document_root: "/srv/api",
    php: "8.3",
    secure: false,
    kind: "linked",
    ...overrides,
  };
}

const header = (s: string, name: string, value: string): ResponseHeaderEntry => ({
  site: s,
  name,
  value,
});

async function mountPanel(s: SiteEntry = site()) {
  const wrapper = mount(SiteHeadersPanel, { props: { site: s } });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  listResponseHeaders.mockReset().mockResolvedValue([]);
  setResponseHeader.mockReset().mockResolvedValue(undefined);
  removeResponseHeader.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("SiteHeadersPanel", () => {
  it("lists only this site's headers, in configured order", async () => {
    listResponseHeaders.mockResolvedValue([
      header("api", "X-Frame-Options", "DENY"),
      header("other", "X-Other", "1"),
      header("api", "Access-Control-Allow-Origin", "https://app.test"),
    ]);
    const wrapper = await mountPanel();
    const rows = wrapper.findAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("X-Frame-Options:");
    expect(rows[0].text()).toContain("DENY");
    expect(rows[1].text()).toContain("Access-Control-Allow-Origin:");
    expect(wrapper.text()).not.toContain("X-Other");
  });

  it("shows an empty state when the site has no headers", async () => {
    const wrapper = await mountPanel();
    expect(wrapper.text()).toContain("No custom response headers for this site.");
  });

  it("sets a header with trimmed values, clears the inputs, and refetches", async () => {
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Header name"]').setValue("  Access-Control-Allow-Origin ");
    await wrapper.find('[aria-label="Header value"]').setValue(" https://app.test  ");
    listResponseHeaders.mockResolvedValue([
      header("api", "Access-Control-Allow-Origin", "https://app.test"),
    ]);

    await wrapper.find("button").trigger("click");
    await flushPromises();

    expect(setResponseHeader).toHaveBeenCalledWith(
      "api",
      "Access-Control-Allow-Origin",
      "https://app.test",
    );
    expect(listResponseHeaders).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalled();
    expect(wrapper.findAll("li")).toHaveLength(1);
    expect((wrapper.find('[aria-label="Header name"]').element as HTMLInputElement).value).toBe("");
    expect((wrapper.find('[aria-label="Header value"]').element as HTMLInputElement).value).toBe("");
  });

  it("keeps Set disabled until both inputs are filled", async () => {
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Header name"]').setValue("X-A");
    await wrapper.find("button").trigger("click");
    await flushPromises();
    expect(setResponseHeader).not.toHaveBeenCalled();
  });

  it("removes a header by its name", async () => {
    listResponseHeaders.mockResolvedValue([header("api", "X-Frame-Options", "DENY")]);
    const wrapper = await mountPanel();

    await wrapper.find('[aria-label="Remove header X-Frame-Options"]').trigger("click");
    await flushPromises();

    expect(removeResponseHeader).toHaveBeenCalledWith("api", "X-Frame-Options");
    expect(toastSuccess).toHaveBeenCalled();
  });

  it("surfaces a daemon rejection as an error toast", async () => {
    setResponseHeader.mockRejectedValue(new Error("Yerd manages this header itself"));
    const wrapper = await mountPanel();
    await wrapper.find('[aria-label="Header name"]').setValue("Content-Length");
    await wrapper.find('[aria-label="Header value"]').setValue("5");

    await wrapper.find("button").trigger("click");
    await flushPromises();

    expect(toastError).toHaveBeenCalledWith(
      "Header change failed",
      "Yerd manages this header itself",
    );
  });

  it("refetches when the sidebar swaps to a different site", async () => {
    const wrapper = await mountPanel();
    expect(listResponseHeaders).toHaveBeenCalledTimes(1);

    await wrapper.setProps({ site: site({ name: "app" }) });
    await flushPromises();

    expect(listResponseHeaders).toHaveBeenCalledTimes(2);
  });
});
