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

import type { ResponseHeaderEntry, SiteEntry } from "@/ipc/types";
import HeaderDialog from "./HeaderDialog.vue";
import SiteHeadersCard from "./SiteHeadersCard.vue";

const site: SiteEntry = { name: "api", document_root: "/srv/api", php: "8.3", secure: false, kind: "linked" };

const header = (s: string, name: string, value: string): ResponseHeaderEntry => ({ site: s, name, value });

async function mountCard() {
  const w = mount(SiteHeadersCard, { props: { site } });
  await flushPromises();
  return w;
}

function rows(w: Awaited<ReturnType<typeof mountCard>>): string[] {
  return w.findAll('[data-testid="header-row"]').map((r) => r.text());
}

beforeEach(() => {
  listResponseHeaders.mockReset().mockResolvedValue([
    header("api", "X-Frame-Options", "DENY"),
    header("other", "X-Other", "1"),
    header("api", "Access-Control-Allow-Origin", "https://app.test"),
  ]);
  setResponseHeader.mockReset().mockResolvedValue(undefined);
  removeResponseHeader.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("SiteHeadersCard", () => {
  it("lists only this site's headers, in order", async () => {
    expect(rows(await mountCard())).toEqual([
      "X-Frame-OptionsDENY",
      "Access-Control-Allow-Originhttps://app.test",
    ]);
  });

  it("shows only Add header when the site has none", async () => {
    listResponseHeaders.mockResolvedValue([]);
    const w = await mountCard();
    expect(rows(w)).toEqual([]);
    expect(w.text()).toContain("Add header");
  });

  it("removes a header and refetches", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Remove header X-Frame-Options"]').trigger("click");
    await flushPromises();
    expect(removeResponseHeader).toHaveBeenCalledWith("api", "X-Frame-Options");
    expect(listResponseHeaders).toHaveBeenCalledTimes(2);
  });

  it("adds through the dialog and closes it", async () => {
    const w = await mountCard();
    await w.findAll("button").find((b) => b.text().includes("Add header"))!.trigger("click");
    const dialog = w.findComponent(HeaderDialog);
    expect(dialog.props("open")).toBe(true);
    expect(dialog.props("editing")).toBeNull();
    dialog.vm.$emit("submit", "X-Debug", "1");
    await flushPromises();
    expect(setResponseHeader).toHaveBeenCalledWith("api", "X-Debug", "1");
    expect(dialog.props("open")).toBe(false);
  });

  it("edits a value in place", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Edit header X-Frame-Options"]').trigger("click");
    const dialog = w.findComponent(HeaderDialog);
    expect(dialog.props("editing")).toEqual(header("api", "X-Frame-Options", "DENY"));
    dialog.vm.$emit("submit", "x-frame-options", "SAMEORIGIN");
    await flushPromises();
    expect(setResponseHeader).toHaveBeenCalledWith("api", "x-frame-options", "SAMEORIGIN");
    expect(removeResponseHeader).not.toHaveBeenCalled();
  });

  it("renames by setting the new name then removing the old", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Edit header X-Frame-Options"]').trigger("click");
    w.findComponent(HeaderDialog).vm.$emit("submit", "Content-Security-Policy", "frame-ancestors 'none'");
    await flushPromises();
    expect(setResponseHeader.mock.invocationCallOrder[0]).toBeLessThan(
      removeResponseHeader.mock.invocationCallOrder[0],
    );
    expect(removeResponseHeader).toHaveBeenCalledWith("api", "X-Frame-Options");
  });

  it("keeps the dialog open and toasts the daemon's message on failure", async () => {
    setResponseHeader.mockRejectedValue(new Error("invalid header name"));
    const w = await mountCard();
    await w.findAll("button").find((b) => b.text().includes("Add header"))!.trigger("click");
    const dialog = w.findComponent(HeaderDialog);
    dialog.vm.$emit("submit", "Bad Name", "1");
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith("Header change failed", "invalid header name");
    expect(dialog.props("open")).toBe(true);
  });

  it("refetches for a different site", async () => {
    const w = await mountCard();
    await w.setProps({ site: { ...site, name: "other" } });
    await flushPromises();
    expect(rows(w)).toEqual(["X-Other1"]);
  });

  it("closes an open dialog when the site switches", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Edit header X-Frame-Options"]').trigger("click");
    const dialog = w.findComponent(HeaderDialog);
    expect(dialog.props("open")).toBe(true);
    await w.setProps({ site: { ...site, name: "other" } });
    expect(dialog.props("open")).toBe(false);
    expect(dialog.props("editing")).toBeNull();
  });

  it("manages a whole-host proxy's headers by name and words the dialog for a proxy", async () => {
    listResponseHeaders.mockResolvedValue([
      header("reverb", "Access-Control-Allow-Origin", "*"),
      header("api", "X-Frame-Options", "DENY"),
    ]);
    const w = mount(SiteHeadersCard, { props: { site: { name: "reverb" }, proxy: true } });
    await flushPromises();
    expect(rows(w)).toEqual(["Access-Control-Allow-Origin*"]);
    expect(w.findComponent(HeaderDialog).props("proxy")).toBe(true);

    await w.findAll("button").find((b) => b.text().includes("Add header"))!.trigger("click");
    w.findComponent(HeaderDialog).vm.$emit("submit", "X-Powered-By", "yerd");
    await flushPromises();
    expect(setResponseHeader).toHaveBeenCalledWith("reverb", "X-Powered-By", "yerd");

    await w.get('[aria-label="Remove header Access-Control-Allow-Origin"]').trigger("click");
    await flushPromises();
    expect(removeResponseHeader).toHaveBeenCalledWith("reverb", "Access-Control-Allow-Origin");
  });
});
