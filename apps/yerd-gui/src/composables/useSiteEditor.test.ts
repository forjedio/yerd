import { flushPromises } from "@vue/test-utils";
import { effectScope, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const getInstalledIdes = vi.fn();
const getPreferredIde = vi.fn();
const getSiteIdeOverrides = vi.fn();
const setSiteIdeOverride = vi.fn();
const openInIde = vi.fn();
const openInSystemDefault = vi.fn();
vi.mock("@/ipc/client", () => ({
  getInstalledIdes: (...a: unknown[]) => getInstalledIdes(...a),
  getPreferredIde: (...a: unknown[]) => getPreferredIde(...a),
  getSiteIdeOverrides: (...a: unknown[]) => getSiteIdeOverrides(...a),
  setSiteIdeOverride: (...a: unknown[]) => setSiteIdeOverride(...a),
  openInIde: (...a: unknown[]) => openInIde(...a),
  openInSystemDefault: (...a: unknown[]) => openInSystemDefault(...a),
  IpcError: class IpcError extends Error {},
}));

const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: vi.fn(), error: toastError }),
}));

import { resetIdes } from "@/composables/useIdes";
import type { SiteEntry } from "@/ipc/types";
import { openSiteInEditor, useSiteEditor } from "./useSiteEditor";

function site(name = "blog"): SiteEntry {
  return { name, document_root: `/srv/${name}`, php: "8.3", secure: true, kind: "linked" };
}

const scopes: ReturnType<typeof effectScope>[] = [];

async function setup(initial: SiteEntry | null = site()) {
  const current = ref<SiteEntry | null>(initial);
  const scope = effectScope();
  scopes.push(scope);
  const editor = scope.run(() => useSiteEditor(() => current.value));
  if (!editor) throw new Error("composable did not run");
  await flushPromises();
  return { current, editor };
}

beforeEach(() => {
  resetIdes();
  getInstalledIdes.mockReset().mockResolvedValue([]);
  getPreferredIde.mockReset().mockResolvedValue(null);
  getSiteIdeOverrides.mockReset().mockResolvedValue({});
  setSiteIdeOverride.mockReset().mockResolvedValue(undefined);
  openInIde.mockReset().mockResolvedValue(undefined);
  openInSystemDefault.mockReset().mockResolvedValue(undefined);
  toastError.mockReset();
});

afterEach(() => {
  scopes.splice(0).forEach((s) => s.stop());
});

describe("useSiteEditor", () => {
  it("opens the folder with the system file manager when no IDE is detected", async () => {
    const { editor } = await setup();
    expect(editor.ideOptions.value.map((o) => o.label)).toEqual([
      "Use default (Open folder)",
      "Open folder",
    ]);
    await editor.openEditor(site());
    expect(openInSystemDefault).toHaveBeenCalledWith("blog");
    expect(openInIde).not.toHaveBeenCalled();
  });

  it("auto-detects the first installed IDE", async () => {
    getInstalledIdes.mockResolvedValue([{ id: "zed", label: "Zed" }]);
    const { editor } = await setup();
    expect(editor.editorLabel.value).toBe("Zed");
    await editor.openEditor(site());
    expect(openInIde).toHaveBeenCalledWith("blog", "zed");
  });

  it("stores a picked override and clears it when the default entry is picked", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "vscode", label: "VS Code" },
      { id: "zed", label: "Zed" },
    ]);
    const { editor } = await setup();
    expect(editor.ideOptions.value.map((o) => o.label)).toEqual([
      "Use default (VS Code)",
      "VS Code",
      "Zed",
      "Open folder",
    ]);
    await editor.changeIde(site(), "zed");
    expect(setSiteIdeOverride).toHaveBeenCalledWith("blog", "zed");
    expect(editor.editorLabel.value).toBe("Zed");
    await editor.changeIde(site(), "default");
    expect(setSiteIdeOverride).toHaveBeenLastCalledWith("blog", null);
  });

  it("honours a stored per-site override and names the global default", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "phpstorm", label: "PhpStorm" },
      { id: "zed", label: "Zed" },
    ]);
    getPreferredIde.mockResolvedValue("zed");
    getSiteIdeOverrides.mockResolvedValue({ blog: "phpstorm" });
    const { editor } = await setup();
    expect(editor.ideOptions.value[0]?.label).toBe("Use default (Zed)");
    expect(editor.selectedIde.value).toBe("phpstorm");
    await editor.openEditor(site());
    expect(openInIde).toHaveBeenCalledWith("blog", "phpstorm");
  });

  it("shows the default entry for an override that is not installed", async () => {
    getInstalledIdes.mockResolvedValue([{ id: "zed", label: "Zed" }]);
    getSiteIdeOverrides.mockResolvedValue({ blog: "sublime" });
    const { editor } = await setup();
    expect(editor.selectedIde.value).toBe("default");
    expect(setSiteIdeOverride).not.toHaveBeenCalled();
    await editor.openEditor(site());
    expect(openInIde).toHaveBeenCalledWith("blog", "zed");
  });

  it("drops the previous site's override while the next site's preferences load", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "phpstorm", label: "PhpStorm" },
      { id: "zed", label: "Zed" },
    ]);
    getSiteIdeOverrides.mockResolvedValue({ blog: "zed" });
    const { current, editor } = await setup();
    expect(editor.selectedIde.value).toBe("zed");

    let release: (overrides: Record<string, string>) => void = () => {};
    getSiteIdeOverrides.mockReturnValue(
      new Promise<Record<string, string>>((resolve) => {
        release = resolve;
      }),
    );
    current.value = site("shop");
    await flushPromises();
    expect(editor.selectedIde.value).toBe("default");
    await editor.openEditor(site("shop"));
    expect(openInIde).toHaveBeenCalledWith("shop", "phpstorm");

    release({ shop: "zed" });
    await flushPromises();
    expect(editor.selectedIde.value).toBe("zed");
  });

  it("does not roll a failed override write back onto another site", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "phpstorm", label: "PhpStorm" },
      { id: "zed", label: "Zed" },
    ]);
    getSiteIdeOverrides.mockResolvedValue({ blog: "zed" });
    const { current, editor } = await setup();

    let fail: (error: unknown) => void = () => {};
    setSiteIdeOverride.mockReturnValue(
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
    );
    const pending = editor.changeIde(site(), "phpstorm");

    getSiteIdeOverrides.mockResolvedValue({});
    current.value = site("shop");
    await flushPromises();

    fail(new Error("write failed"));
    await pending;
    expect(editor.selectedIde.value).toBe("default");
  });

  it("rolls a failed write back on the same site", async () => {
    getInstalledIdes.mockResolvedValue([{ id: "zed", label: "Zed" }]);
    getSiteIdeOverrides.mockResolvedValue({ blog: "zed" });
    setSiteIdeOverride.mockRejectedValue(new Error("write failed"));
    const { editor } = await setup();
    await editor.changeIde(site(), "system");
    expect(editor.selectedIde.value).toBe("zed");
    expect(toastError).toHaveBeenCalledWith("Couldn't change the editor", "write failed");
  });

  it("loads nothing while no site is in view", async () => {
    await setup(null);
    expect(getSiteIdeOverrides).not.toHaveBeenCalled();
  });
});

describe("openSiteInEditor", () => {
  it("resolves the site's override over the global preference", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "phpstorm", label: "PhpStorm" },
      { id: "zed", label: "Zed" },
    ]);
    getPreferredIde.mockResolvedValue("zed");
    getSiteIdeOverrides.mockResolvedValue({ blog: "phpstorm" });
    await openSiteInEditor("blog");
    expect(openInIde).toHaveBeenCalledWith("blog", "phpstorm");
  });

  it("falls back to the system default and toasts a failure", async () => {
    openInSystemDefault.mockRejectedValue(new Error("no handler"));
    await openSiteInEditor("blog");
    expect(openInSystemDefault).toHaveBeenCalledWith("blog");
    expect(toastError).toHaveBeenCalledWith("Couldn't open the site folder", "no handler");
  });
});
