import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const nodeSite = vi.fn();
const listNode = vi.fn();
const setNodeVersion = vi.fn();
const clearNodeVersion = vi.fn();
const installNodeWithProgress = vi.fn();
vi.mock("@/ipc/client", () => ({
  installNodeWithProgress: (...a: unknown[]) => installNodeWithProgress(...a),
  nodeSite: (...a: unknown[]) => nodeSite(...a),
  listNode: (...a: unknown[]) => listNode(...a),
  setNodeVersion: (...a: unknown[]) => setNodeVersion(...a),
  clearNodeVersion: (...a: unknown[]) => clearNodeVersion(...a),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
}));

import SiteNodePanel from "./SiteNodePanel.vue";
import { useOperations } from "@/composables/useOperations";
import { resetResourceCache } from "@/composables/useResource";
import type { NodeRelease, NodeSiteResponse, SiteEntry } from "@/ipc/types";

function site(name = "blog"): SiteEntry {
  return {
    name,
    document_root: `/srv/${name}`,
    php: "8.4",
    secure: false,
    kind: "linked",
  };
}

function release(version: string): NodeRelease {
  return { version, lts: true, default: false, uninstall_blocked: false };
}

function reply(fields: Omit<NodeSiteResponse, "type"> = {}): NodeSiteResponse {
  return { type: "node_site", ...fields };
}

async function mountPanel(s: SiteEntry = site()) {
  const wrapper = mount(SiteNodePanel, { props: { site: s } });
  await flushPromises();
  return wrapper;
}

function select(wrapper: Awaited<ReturnType<typeof mountPanel>>) {
  return wrapper.get('select[aria-label="Site Node version"]');
}

function segment(wrapper: Awaited<ReturnType<typeof mountPanel>>, label: string) {
  const radio = wrapper.findAll('[role="radio"]').find((r) => r.text() === label);
  if (!radio) throw new Error(`no ${label} segment`);
  return radio;
}

beforeEach(() => {
  resetResourceCache();
  nodeSite.mockReset().mockResolvedValue(reply());
  listNode.mockReset().mockResolvedValue([release("22.9.0"), release("20.18.0")]);
  setNodeVersion.mockReset().mockResolvedValue(undefined);
  clearNodeVersion.mockReset().mockResolvedValue(undefined);
  installNodeWithProgress.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
  push.mockReset();
});

afterEach(() => {
  const ops = useOperations();
  ops.active.value.forEach((o) => ops.end(o.id));
});

describe("SiteNodePanel", () => {
  it("loads the site and explains how to use its version", async () => {
    nodeSite.mockResolvedValue(reply({ effective: "22.9.0", source: "nvmrc" }));
    const wrapper = await mountPanel();
    expect(nodeSite).toHaveBeenCalledWith("blog");
    expect(wrapper.text()).not.toContain("Effective");
    expect(wrapper.get('[data-testid="site-node-hint"]').attributes("title")).toContain(
      "yerd exec npm",
    );
  });

  it.each([
    [{ preference: "22.9.0", effective: "22.9.0", source: "site" }, "Saved preference · builds use Node 22.9.0"],
    [
      { project_file: { kind: "nvmrc", selector: "20" }, effective: "20.18.0", source: "nvmrc" },
      "Project file .nvmrc · builds use Node 20.18.0",
    ],
    [{ effective: "22.9.0", source: "default" }, "Global default · builds use Node 22.9.0"],
    [{ error: "Node 18 is not installed" }, ""],
  ] as const)("subtitles %o as %s", async (fields, expected) => {
    nodeSite.mockResolvedValue(reply(fields as Omit<NodeSiteResponse, "type">));
    const wrapper = await mountPanel();
    expect(wrapper.get('[data-testid="site-node-subtitle"]').text()).toBe(expected);
  });

  it("offers Default and each release's major as segments", async () => {
    const wrapper = await mountPanel();
    expect(wrapper.findAll('[role="radio"]').map((r) => r.text())).toEqual(["Default", "22", "20"]);
    expect(wrapper.find("select").exists()).toBe(false);
  });

  it("falls back to a select above four releases", async () => {
    listNode.mockResolvedValue(["24.1.0", "22.9.0", "20.18.0", "18.20.4", "16.20.2"].map(release));
    const wrapper = await mountPanel();
    expect(select(wrapper).findAll("option")).toHaveLength(6);
  });

  it("shows the precedence banner only when a project file and a preference both exist", async () => {
    nodeSite.mockResolvedValue(
      reply({
        preference: "22.9.0",
        project_file: { kind: "nvmrc", selector: "18" },
        error: "Node 18 is not installed",
      }),
    );
    const both = await mountPanel();
    const banner = both.get('[data-testid="site-node-precedence"]').text();
    expect(banner).toContain(".nvmrc");
    expect(banner).toContain("(18)");
    expect(banner).toContain("(22.9.0)");

    nodeSite.mockResolvedValue(
      reply({ project_file: { kind: "node-version", selector: "20" }, effective: "20.18.0", source: "node-version" }),
    );
    const fileOnly = await mountPanel();
    expect(fileOnly.find('[data-testid="site-node-precedence"]').exists()).toBe(false);

    nodeSite.mockResolvedValue(reply({ preference: "20.18.0", effective: "20.18.0", source: "site" }));
    const prefOnly = await mountPanel();
    expect(prefOnly.find('[data-testid="site-node-precedence"]').exists()).toBe(false);
  });

  it("sets a preference for this site and reloads", async () => {
    const wrapper = await mountPanel();
    expect(nodeSite).toHaveBeenCalledTimes(1);
    await segment(wrapper, "20").trigger("click");
    await flushPromises();
    expect(setNodeVersion).toHaveBeenCalledWith("20.18.0", "blog");
    expect(clearNodeVersion).not.toHaveBeenCalled();
    expect(nodeSite).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalled();
    expect(wrapper.emitted("changed")).toHaveLength(1);
  });

  it("emits no change when the write fails", async () => {
    setNodeVersion.mockRejectedValue(new Error("nope"));
    const wrapper = await mountPanel();
    await segment(wrapper, "20").trigger("click");
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith("Couldn't change the Node version", "nope");
    expect(wrapper.emitted("changed")).toBeUndefined();
  });

  it("clears the preference when the default option is picked", async () => {
    nodeSite.mockResolvedValue(reply({ preference: "22.9.0", effective: "22.9.0", source: "site" }));
    const wrapper = await mountPanel();
    expect(segment(wrapper, "22").attributes("aria-checked")).toBe("true");
    await segment(wrapper, "Default").trigger("click");
    await flushPromises();
    expect(clearNodeVersion).toHaveBeenCalledWith("blog");
    expect(setNodeVersion).not.toHaveBeenCalled();
    expect(wrapper.emitted("changed")).toHaveLength(1);
  });

  it("offers a saved preference that is not an installed release", async () => {
    nodeSite.mockResolvedValue(reply({ preference: "18", error: "Node 18 is not installed" }));
    const wrapper = await mountPanel();
    const labels = select(wrapper)
      .findAll("option")
      .map((o) => o.text());
    expect(labels).toEqual(["Default", "22.9.0", "20.18.0", "18 (saved)"]);
  });

  it("renders the error with a link to the Node view", async () => {
    nodeSite.mockResolvedValue(reply({ error: "Node 18 is not installed" }));
    const wrapper = await mountPanel();
    const err = wrapper.get('[data-testid="site-node-error"]');
    expect(err.text()).toContain("Node 18 is not installed");
    const link = err.findAll("button").find((b) => b.text() === "Manage Node versions");
    await link?.trigger("click");
    expect(push).toHaveBeenCalledWith("/node");
  });

  it("hides the error link when the site resolves", async () => {
    nodeSite.mockResolvedValue(reply({ effective: "22.9.0", source: "default" }));
    const wrapper = await mountPanel();
    expect(wrapper.find('[data-testid="site-node-error"]').exists()).toBe(false);
  });

  it("disables the picker while a Node install runs", async () => {
    useOperations().begin({ id: "node-install:22", kind: "tool-install", label: "Installing" });
    const wrapper = await mountPanel();
    expect(segment(wrapper, "20").attributes("disabled")).toBeDefined();
  });

  it("disables the picker with a note when no release is installed", async () => {
    listNode.mockResolvedValue([]);
    const wrapper = await mountPanel();
    expect(segment(wrapper, "Default").attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("No Node release installed");
  });

  it("ignores a stale response after a quick site switch", async () => {
    let resolveBlog: (r: NodeSiteResponse) => void = () => {};
    nodeSite.mockImplementation((name: string) =>
      name === "blog"
        ? new Promise<NodeSiteResponse>((resolve) => {
            resolveBlog = resolve;
          })
        : Promise.resolve(reply({ error: "shop: Node 18 is not installed" })),
    );
    const wrapper = mount(SiteNodePanel, { props: { site: site("blog") } });
    await wrapper.setProps({ site: site("shop") });
    await flushPromises();
    resolveBlog(reply({ error: "blog: Node 16 is not installed" }));
    await flushPromises();
    expect(wrapper.get('[data-testid="site-node-error"]').text()).toContain("shop:");
  });

  it("installs the project file's missing version in place", async () => {
    nodeSite.mockResolvedValue(
      reply({
        preference: "20.18.0",
        project_file: { kind: "nvmrc", selector: "18" },
        error: "Node 18 is not installed - run `yerd install tool node 18`",
        missing: "18",
      }),
    );
    const wrapper = await mountPanel();
    const install = wrapper.get('[data-testid="site-node-install"]');
    expect(install.text()).toContain("Install Node 18");
    nodeSite.mockResolvedValue(reply({ effective: "18.20.4", source: "nvmrc" }));
    await install.trigger("click");
    await flushPromises();
    expect(installNodeWithProgress.mock.calls[0]?.slice(0, 2)).toEqual(["18", false]);
    expect(toastSuccess).toHaveBeenCalledWith("Installed Node 18");
    expect(wrapper.emitted("changed")).toHaveLength(1);
    expect(wrapper.find('[data-testid="site-node-error"]').exists()).toBe(false);
  });

  it("offers to install a missing global default", async () => {
    nodeSite.mockResolvedValue(reply({ error: "Node 99 is not installed", missing: "99" }));
    const wrapper = await mountPanel();
    expect(wrapper.get('[data-testid="site-node-install"]').text()).toContain("Install Node 99");
  });

  it("offers no install for an error that is not a missing release", async () => {
    nodeSite.mockResolvedValue(reply({ error: "invalid .nvmrc: expected a version" }));
    const wrapper = await mountPanel();
    expect(wrapper.find('[data-testid="site-node-install"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("Manage Node versions");
  });

  it("writes nothing when the selected segment is clicked again", async () => {
    nodeSite.mockResolvedValue(reply({ preference: "22.9.0", effective: "22.9.0", source: "site" }));
    const wrapper = await mountPanel();
    await segment(wrapper, "22").trigger("click");
    await flushPromises();
    nodeSite.mockResolvedValue(reply());
    const unset = await mountPanel();
    await segment(unset, "Default").trigger("click");
    await flushPromises();
    expect(setNodeVersion).not.toHaveBeenCalled();
    expect(clearNodeVersion).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  it("keeps another site's picker usable while a change is in flight", async () => {
    setNodeVersion.mockReturnValue(new Promise(() => {}));
    const wrapper = await mountPanel(site("blog"));
    await segment(wrapper, "20").trigger("click");
    await flushPromises();
    expect(segment(wrapper, "20").attributes("disabled")).toBeDefined();
    await wrapper.setProps({ site: site("shop") });
    await flushPromises();
    expect(segment(wrapper, "20").attributes("disabled")).toBeUndefined();
  });

  it("shows install progress only under the site that started it", async () => {
    let finish: () => void = () => {};
    installNodeWithProgress.mockImplementation(
      (_v: string, _d: boolean, onLines: (lines: string[]) => void) =>
        new Promise<void>((resolve) => {
          onLines(["downloading node 18"]);
          finish = resolve;
        }),
    );
    nodeSite.mockResolvedValue(reply({ error: "Node 18 is not installed", missing: "18" }));
    const wrapper = await mountPanel(site("blog"));
    await wrapper.get('[data-testid="site-node-install"]').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("downloading node 18");
    await wrapper.setProps({ site: site("shop") });
    await flushPromises();
    expect(wrapper.text()).not.toContain("downloading node 18");
    finish();
    await flushPromises();
  });

  it("ignores a reply that lands after the panel closes", async () => {
    let fail: (e: unknown) => void = () => {};
    nodeSite.mockImplementation(
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    const wrapper = mount(SiteNodePanel, { props: { site: site() } });
    wrapper.unmount();
    fail(new Error("daemon went away"));
    await flushPromises();
    expect(toastError).not.toHaveBeenCalled();
  });
});
