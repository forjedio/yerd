import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetSitesGroupState, useSitesGroupState } from "./sitesGroupState";

const STORAGE_KEY = "yerd.sites.collapsedGroups";

/** Minimal in-memory localStorage - the jsdom unit env doesn't provide one
 *  (theme.ts's try/catch relies on exactly this being absent in production). */
function stubLocalStorage(): Map<string, string> {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  });
  return store;
}

describe("sitesGroupState", () => {
  beforeEach(() => {
    stubLocalStorage();
    resetSitesGroupState();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("defaults to nothing collapsed", () => {
    const { isCollapsed } = useSitesGroupState();
    expect(isCollapsed("Blog")).toBe(false);
  });

  it("toggles and persists collapse state to localStorage", () => {
    const { toggle, isCollapsed } = useSitesGroupState();
    toggle("Blog");
    expect(isCollapsed("Blog")).toBe(true);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]")).toEqual(["Blog"]);

    toggle("Blog");
    expect(isCollapsed("Blog")).toBe(false);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]")).toEqual([]);
  });

  it("setCollapsed is idempotent per name", () => {
    const { setCollapsed, isCollapsed } = useSitesGroupState();
    setCollapsed("Shop", true);
    setCollapsed("Shop", true);
    expect(isCollapsed("Shop")).toBe(true);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]")).toEqual(["Shop"]);
  });

  it("carries collapsed state across a rename", () => {
    const { toggle, rename, isCollapsed } = useSitesGroupState();
    toggle("Blog");
    rename("Blog", "Journal");
    expect(isCollapsed("Blog")).toBe(false);
    expect(isCollapsed("Journal")).toBe(true);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]")).toEqual(["Journal"]);
  });

  it("rename is a no-op when the source group wasn't collapsed", () => {
    const { rename, isCollapsed } = useSitesGroupState();
    rename("Blog", "Journal");
    expect(isCollapsed("Journal")).toBe(false);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]")).toEqual([]);
  });

  it("seeds collapsed set from existing localStorage on fresh import", async () => {
    const store = stubLocalStorage();
    store.set(STORAGE_KEY, JSON.stringify(["Blog", "Shop"]));
    vi.resetModules();
    const mod = await import("./sitesGroupState");
    const { isCollapsed } = mod.useSitesGroupState();
    expect(isCollapsed("Blog")).toBe(true);
    expect(isCollapsed("Shop")).toBe(true);
  });
});
