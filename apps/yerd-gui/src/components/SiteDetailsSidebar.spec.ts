import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const openInBrowser = vi.fn();
const openPath = vi.fn();
const openInTerminal = vi.fn();
const openInIde = vi.fn();
const openInSystemDefault = vi.fn();
const getInstalledIdes = vi.fn();
const pickDirectory = vi.fn();
const showDumpsWindow = vi.fn();
const mintWordPressLoginToken = vi.fn();
const wordpressAdminUsers = vi.fn();
const addDomain = vi.fn();
const removeDomain = vi.fn();
const setPrimaryDomain = vi.fn();
const resetDomains = vi.fn();
const listRoutes = vi.fn();
const addRouteRule = vi.fn();
const removeRouteRule = vi.fn();
const clipboardWriteText = vi.fn();
vi.mock("@/ipc/client", () => ({
  openInBrowser: (...args: unknown[]) => openInBrowser(...args),
  openPath: (...args: unknown[]) => openPath(...args),
  openInTerminal: (...args: unknown[]) => openInTerminal(...args),
  openInIde: (...args: unknown[]) => openInIde(...args),
  openInSystemDefault: (...args: unknown[]) => openInSystemDefault(...args),
  getInstalledIdes: (...args: unknown[]) => getInstalledIdes(...args),
  pickDirectory: (...args: unknown[]) => pickDirectory(...args),
  showDumpsWindow: (...args: unknown[]) => showDumpsWindow(...args),
  mintWordPressLoginToken: (...args: unknown[]) => mintWordPressLoginToken(...args),
  wordpressAdminUsers: (...args: unknown[]) => wordpressAdminUsers(...args),
  addDomain: (...args: unknown[]) => addDomain(...args),
  removeDomain: (...args: unknown[]) => removeDomain(...args),
  setPrimaryDomain: (...args: unknown[]) => setPrimaryDomain(...args),
  resetDomains: (...args: unknown[]) => resetDomains(...args),
  listRoutes: (...args: unknown[]) => listRoutes(...args),
  addRouteRule: (...args: unknown[]) => addRouteRule(...args),
  removeRouteRule: (...args: unknown[]) => removeRouteRule(...args),
  IpcError: class IpcError extends Error {},
}));

import type { SiteEntry, StatusReport } from "@/ipc/types";
import SiteDetailsSidebar from "./SiteDetailsSidebar.vue";

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "blog",
    document_root: "/srv/blog",
    php: "8.3",
    secure: true,
    kind: "linked",
    is_laravel: true,
    uses_front_controller: true,
    ...overrides,
  };
}

function wpSite(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return site({
    is_laravel: false,
    is_wordpress: true,
    uses_front_controller: undefined,
    wp_auto_login: false,
    ...overrides,
  });
}

/** Switches the sidebar to a tab by its visible label. */
async function openTab(wrapper: ReturnType<typeof mountSidebar>, label: string) {
  const tab = wrapper.findAll('[role="tab"]').find((t) => t.text() === label);
  if (!tab) throw new Error(`${label} tab not rendered`);
  await tab.trigger("click");
}

function report(): StatusReport {
  return {
    tld: "test",
    resolver_installed: true,
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
  } as StatusReport;
}

function mountSidebar(s: SiteEntry = site(), extraProps: Record<string, unknown> = {}) {
  return mount(SiteDetailsSidebar, {
    props: {
      site: s,
      open: true,
      report: report(),
      tld: "test",
      phpVersions: ["8.3", "8.4"],
      ...extraProps,
    },
    global: { stubs: { teleport: true } },
  });
}

describe("SiteDetailsSidebar", () => {
  beforeEach(() => {
    openInBrowser.mockReset();
    openPath.mockReset();
    openInTerminal.mockReset();
    openInIde.mockReset();
    openInSystemDefault.mockReset();
    getInstalledIdes.mockReset().mockResolvedValue([]);
    pickDirectory.mockReset();
    showDumpsWindow.mockReset();
    mintWordPressLoginToken.mockReset();
    wordpressAdminUsers.mockReset().mockResolvedValue([]);
    addDomain.mockReset().mockResolvedValue(undefined);
    removeDomain.mockReset().mockResolvedValue(undefined);
    setPrimaryDomain.mockReset().mockResolvedValue(undefined);
    resetDomains.mockReset().mockResolvedValue(undefined);
    listRoutes.mockReset().mockResolvedValue([]);
    addRouteRule.mockReset().mockResolvedValue(undefined);
    removeRouteRule.mockReset().mockResolvedValue(undefined);
    clipboardWriteText.mockReset();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: clipboardWriteText },
    });
  });

  it("renders site information and opens the site", async () => {
    const wrapper = mountSidebar();

    expect(wrapper.text()).toContain("blog.test");
    expect(wrapper.text()).toContain("/srv/blog");
    expect(wrapper.text()).toContain("Laravel");
    expect(wrapper.text()).toContain("8.3");
    expect(wrapper.text()).toContain("IDE switch");
    expect(wrapper.text()).not.toContain("Tinker");
    expect(wrapper.text()).toContain("Terminal");
    expect(wrapper.text()).toContain("Dumps");
    expect(wrapper.text()).not.toContain("Edit site");

    const openButton = wrapper.findAll("button").find((button) => button.text() === "Open site");
    if (!openButton) throw new Error("Open site button not rendered");
    await openButton.trigger("click");

    expect(openInBrowser).toHaveBeenCalledWith("https://blog.test");
  });

  it("opens site actions and converts a picked web root to a relative path", async () => {
    const wrapper = mountSidebar();
    pickDirectory.mockResolvedValue("/srv/blog/public");

    await wrapper.get('[aria-label="Choose web root directory"]').trigger("click");
    expect(pickDirectory).toHaveBeenCalledWith("/srv/blog");
    expect((wrapper.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe(
      "public",
    );

    await wrapper.get('[title="Reveal /srv/blog"]').trigger("click");
    expect(openPath).toHaveBeenCalledWith("/srv/blog");

    const terminal = wrapper.findAll("button").find((button) => button.text().includes("Terminal"));
    if (!terminal) throw new Error("Terminal button not rendered");
    await terminal.trigger("click");
    expect(openInTerminal).toHaveBeenCalledWith("/srv/blog");

    const dumps = wrapper.findAll("button").find((button) => button.text().includes("Dumps"));
    if (!dumps) throw new Error("Dumps button not rendered");
    await dumps.trigger("click");
    expect(showDumpsWindow).toHaveBeenCalledOnce();
  });

  it("opens the site folder with the system file manager when no IDE is detected", async () => {
    const wrapper = mountSidebar();
    await flushPromises();

    const editor = wrapper.findAll("button").find((button) => button.text() === "Open folder");
    if (!editor) throw new Error("Editor button not rendered");
    await editor.trigger("click");

    expect(openInSystemDefault).toHaveBeenCalledWith("/srv/blog");
    expect(openInIde).not.toHaveBeenCalled();
    expect(
      wrapper.get('[aria-label="Site IDE"]').findAll("option").map((option) => option.text()),
    ).toEqual(["Auto-detect"]);
  });

  it("opens the site folder with the selected detected IDE", async () => {
    getInstalledIdes.mockResolvedValue([
      { id: "vscode", label: "VS Code" },
      { id: "zed", label: "Zed" },
    ]);
    const wrapper = mountSidebar();
    await flushPromises();

    expect(
      wrapper.get('[aria-label="Site IDE"]').findAll("option").map((option) => option.text()),
    ).toEqual(["Auto-detect", "VS Code", "Zed"]);
    await wrapper.get('[aria-label="Site IDE"]').setValue("zed");
    const editor = wrapper.findAll("button").find((button) => button.text() === "Zed");
    if (!editor) throw new Error("IDE button not rendered");
    await editor.trigger("click");

    expect(openInIde).toHaveBeenCalledWith("/srv/blog", "zed");
  });

  it("auto-detects the first installed IDE", async () => {
    getInstalledIdes.mockResolvedValue([{ id: "zed", label: "Zed" }]);
    const wrapper = mountSidebar();
    await flushPromises();

    const editor = wrapper.findAll("button").find((button) => button.text() === "Zed");
    if (!editor) throw new Error("Zed button not rendered");
    await editor.trigger("click");

    expect(openInIde).toHaveBeenCalledWith("/srv/blog", "zed");
    expect(openInSystemDefault).not.toHaveBeenCalled();
  });

  it("rejects a picked directory outside the site folder", async () => {
    const wrapper = mountSidebar();
    pickDirectory.mockResolvedValue("/srv/other");

    await wrapper.get('[aria-label="Choose web root directory"]').trigger("click");

    expect((wrapper.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe(
      "",
    );
  });

  it("copies the current web root", async () => {
    const wrapper = mountSidebar();

    await wrapper.get('[aria-label="Copy web root"]').trigger("click");

    expect(clipboardWriteText).toHaveBeenCalledWith("");
  });

  it("drops an unsaved web root edit when pointed at another site", async () => {
    const wrapper = mountSidebar();

    await wrapper.get('[aria-label="Site web root"]').setValue("public");
    await wrapper.setProps({ site: site({ name: "shop", document_root: "/srv/shop" }) });

    expect((wrapper.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe(
      "",
    );
    expect(wrapper.findAll("button").find((button) => button.text() === "Save")).toBeUndefined();
  });

  it("drops an unsaved web root edit when reopened on the same site", async () => {
    const wrapper = mountSidebar();

    await wrapper.get('[aria-label="Site web root"]').setValue("public");
    await wrapper.setProps({ open: false });
    await wrapper.setProps({ open: true });

    expect((wrapper.get('[aria-label="Site web root"]').element as HTMLInputElement).value).toBe(
      "",
    );
  });

  it("closes when the backdrop is clicked", async () => {
    const wrapper = mountSidebar();

    await wrapper.get(".site-sidebar-backdrop").trigger("click");

    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("keeps every tab connected to the rendered panel", async () => {
    const wrapper = mountSidebar();

    for (const tab of wrapper.findAll('[role="tab"]')) {
      await tab.trigger("click");
      const panel = wrapper.get('[role="tabpanel"]');
      const tabId = tab.attributes("id");
      const tabName = tabId?.replace("site-details-tab-", "");

      expect(panel.attributes("id")).toBe(`site-details-panel-${tabName}`);
      expect(tab.attributes("aria-controls")).toBe(panel.attributes("id"));
      expect(panel.attributes("aria-labelledby")).toBe(tabId);
    }
  });

  it("closes on Escape", async () => {
    const wrapper = mountSidebar();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("leaves Escape to a dialog layered above it", async () => {
    const wrapper = mountSidebar();
    const above = document.createElement("div");
    above.setAttribute("role", "dialog");
    above.setAttribute("aria-modal", "true");
    document.body.appendChild(above);

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(wrapper.emitted("close")).toBeUndefined();

    above.remove();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("provides General controls and keeps application details under Information", async () => {
    const wrapper = mountSidebar();

    const php = wrapper.get('[aria-label="Site PHP version"]');
    await php.setValue("8.4");
    expect(wrapper.emitted("changePhp")).toEqual([[site(), "8.4"]]);

    const webRoot = wrapper.get('[aria-label="Site web root"]');
    await webRoot.setValue("public");
    const saveWebRoot = wrapper.findAll("button").find((button) => button.text() === "Save");
    if (!saveWebRoot) throw new Error("Save web root button not rendered");
    await saveWebRoot.trigger("click");
    expect(wrapper.emitted("changeWebRoot")).toEqual([[site(), "public"]]);

    const frontController = wrapper.get('[aria-label="Route through front controller"]');
    await frontController.trigger("click");
    expect(wrapper.emitted("toggleFrontController")).toEqual([[site(), false]]);

    const https = wrapper.get('[aria-label="HTTPS"]');
    await https.trigger("click");
    expect(wrapper.emitted("toggleSecure")).toEqual([[site()]]);

    const information = wrapper.findAll('[role="tab"]').find((tab) => tab.text() === "Information");
    if (!information) throw new Error("Information tab not rendered");
    await information.trigger("click");

    expect(wrapper.text()).toContain("Application");
    expect(wrapper.text()).not.toContain("PHP version");
  });

  it("manages domains from the Domains tab", async () => {
    const wrapper = mountSidebar(site({ domains: ["blog.test", "api.blog.test"] }));

    await openTab(wrapper, "Domains");
    expect(wrapper.findAll("li")).toHaveLength(2);

    await wrapper.get("#add-domain").setValue("shop.blog.test");
    const add = wrapper.findAll("button").find((button) => button.text().includes("Add"));
    if (!add) throw new Error("Add domain button not rendered");
    await add.trigger("click");
    await flushPromises();

    expect(addDomain).toHaveBeenCalledWith("blog", "shop.blog.test");
    expect(wrapper.emitted("domainsChanged")).toHaveLength(1);
  });

  it("manages routing rules from the Routing tab", async () => {
    listRoutes.mockResolvedValue([{ site: "blog", prefix: "/api", target: "api/index.php" }]);
    const wrapper = mountSidebar();

    await openTab(wrapper, "Routing");
    await flushPromises();
    expect(wrapper.text()).toContain("/api");
    expect(wrapper.text()).toContain("api/index.php");

    await wrapper.get('[aria-label="Remove route /api"]').trigger("click");
    await flushPromises();
    expect(removeRouteRule).toHaveBeenCalledWith("blog", "/api");
  });

  it("returns to the General tab when reopened", async () => {
    const wrapper = mountSidebar();

    await openTab(wrapper, "Domains");
    await wrapper.setProps({ open: false });
    await wrapper.setProps({ open: true });

    expect(wrapper.text()).toContain("PHP version");
  });

  it("offers the group picker only when groups exist", async () => {
    const withoutGroups = mountSidebar();
    expect(withoutGroups.find('[aria-label="Site group"]').exists()).toBe(false);

    const s = site();
    const wrapper = mountSidebar(s, {
      groupOptions: [
        { value: "", label: "No group" },
        { value: "Client work", label: "Client work" },
      ],
      currentGroup: "",
    });

    await wrapper.get('[aria-label="Site group"]').setValue("Client work");

    expect(wrapper.emitted("changeGroup")).toEqual([[s, "Client work"]]);
  });

  it("shares and unlinks from the sidebar", async () => {
    const s = site();
    const wrapper = mountSidebar(s);

    const share = wrapper.findAll("button").find((b) => b.text().includes("Share publicly"));
    if (!share) throw new Error("Share button not rendered");
    await share.trigger("click");
    expect(wrapper.emitted("share")).toEqual([[s]]);

    const unlinkButton = wrapper.findAll("button").find((b) => b.text() === "Unlink");
    if (!unlinkButton) throw new Error("Unlink button not rendered");
    await unlinkButton.trigger("click");
    expect(wrapper.emitted("unlink")).toEqual([[s]]);
  });

  it("hides Unlink for a parked site, which is removed by un-parking its folder", () => {
    const wrapper = mountSidebar(site({ kind: "parked" }));

    expect(wrapper.findAll("button").find((b) => b.text() === "Unlink")).toBeUndefined();
  });

  it("hides WordPress-only controls on a non-WordPress site", () => {
    const wrapper = mountSidebar();

    expect(wrapper.findAll("button").find((b) => b.text().includes("WP Admin"))).toBeUndefined();
    expect(wrapper.find('[aria-label="WordPress Auto Admin Login"]').exists()).toBe(false);
    expect(wordpressAdminUsers).not.toHaveBeenCalled();
  });
});

describe("SiteDetailsSidebar WordPress controls", () => {
  beforeEach(() => {
    openInBrowser.mockReset();
    mintWordPressLoginToken.mockReset();
    wordpressAdminUsers.mockReset().mockResolvedValue([]);
  });

  it("opens the plain WP Admin link when auto-login is off", async () => {
    const wrapper = mountSidebar(wpSite({ wp_auto_login: false }));

    const wpAdmin = wrapper.findAll("button").find((b) => b.text().includes("WP Admin"));
    if (!wpAdmin) throw new Error("WP Admin button not rendered");
    await wpAdmin.trigger("click");
    await flushPromises();

    expect(mintWordPressLoginToken).not.toHaveBeenCalled();
    expect(openInBrowser).toHaveBeenCalledWith("https://blog.test/wp-admin/");
  });

  it("mints a pre-authenticated WP Admin link when auto-login is on", async () => {
    mintWordPressLoginToken.mockResolvedValue("sekrit-token");
    const wrapper = mountSidebar(wpSite({ wp_auto_login: true }));

    const wpAdmin = wrapper.findAll("button").find((b) => b.text().includes("WP Admin"));
    if (!wpAdmin) throw new Error("WP Admin button not rendered");
    await wpAdmin.trigger("click");
    await flushPromises();

    expect(openInBrowser).toHaveBeenCalledWith(
      "https://blog.test/wp-admin/?yerd_login_token=sekrit-token",
    );
  });

  it("toggles auto-login and changes the signed-in user", async () => {
    wordpressAdminUsers.mockResolvedValue([{ login: "editor", display_name: "Editor" }]);
    const off = wpSite({ wp_auto_login: false });
    const wrapper = mountSidebar(off);

    await wrapper.get('[aria-label="WordPress Auto Admin Login"]').trigger("click");
    expect(wrapper.emitted("changeWpAutoLogin")).toEqual([[off, true, null]]);

    const on = wpSite({ wp_auto_login: true });
    await wrapper.setProps({ site: on });
    await flushPromises();

    await wrapper.get('[aria-label="Sign in as"]').setValue("editor");
    expect(wrapper.emitted("changeWpAutoLogin")?.[1]).toEqual([on, true, "editor"]);
  });
});
