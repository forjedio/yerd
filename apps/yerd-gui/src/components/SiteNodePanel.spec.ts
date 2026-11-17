import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const nodeSite = vi.fn();
const listNode = vi.fn();
const setNodeVersion = vi.fn();
const clearNodeVersion = vi.fn();
vi.mock("@/ipc/client", () => ({
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

beforeEach(() => {
  resetResourceCache();
  nodeSite.mockReset().mockResolvedValue(reply());
  listNode.mockReset().mockResolvedValue([release("22.9.0"), release("20.18.0")]);
  setNodeVersion.mockReset().mockResolvedValue(undefined);
  clearNodeVersion.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
  push.mockReset();
});

afterEach(() => {
  const ops = useOperations();
  ops.active.value.forEach((o) => ops.end(o.id));
});

describe("SiteNodePanel", () => {
  it.each([
    ["nvmrc", "from .nvmrc"],
    ["node-version", "from .node-version"],
    ["site", "from site preference"],
    ["default", "from global default"],
  ] as const)("labels the %s source", async (source, label) => {
    nodeSite.mockResolvedValue(reply({ effective: "22.9.0", source }));
    const wrapper = await mountPanel();
    const text = wrapper.get('[data-testid="site-node-effective"]').text();
    expect(text).toContain("22.9.0");
    expect(text).toContain(label);
    expect(nodeSite).toHaveBeenCalledWith("blog");
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
    await select(wrapper).setValue("20.18.0");
    await flushPromises();
    expect(setNodeVersion).toHaveBeenCalledWith("20.18.0", "blog");
    expect(clearNodeVersion).not.toHaveBeenCalled();
    expect(nodeSite).toHaveBeenCalledTimes(2);
    expect(toastSuccess).toHaveBeenCalled();
  });

  it("clears the preference when the default option is picked", async () => {
    nodeSite.mockResolvedValue(reply({ preference: "22.9.0", effective: "22.9.0", source: "site" }));
    const wrapper = await mountPanel();
    expect((select(wrapper).element as HTMLSelectElement).value).toBe("22.9.0");
    await select(wrapper).setValue("");
    await flushPromises();
    expect(clearNodeVersion).toHaveBeenCalledWith("blog");
    expect(setNodeVersion).not.toHaveBeenCalled();
  });

  it("offers a saved preference that is not an installed release", async () => {
    nodeSite.mockResolvedValue(reply({ preference: "18", error: "Node 18 is not installed" }));
    const wrapper = await mountPanel();
    const labels = select(wrapper)
      .findAll("option")
      .map((o) => o.text());
    expect(labels).toEqual(["Use project file or global default", "22.9.0", "20.18.0", "18 (saved)"]);
  });

  it("renders the error with a link to the Node view", async () => {
    nodeSite.mockResolvedValue(reply({ error: "Node 18 is not installed" }));
    const wrapper = await mountPanel();
    const err = wrapper.get('[data-testid="site-node-error"]');
    expect(err.text()).toContain("Node 18 is not installed");
    const link = err.findAll("button").find((b) => b.text() === "Manage Node versions");
    await link?.trigger("click");
    expect(push).toHaveBeenCalledWith("/node");
    expect(wrapper.find('[data-testid="site-node-effective"]').exists()).toBe(false);
  });

  it("hides the error link when the site resolves", async () => {
    nodeSite.mockResolvedValue(reply({ effective: "22.9.0", source: "default" }));
    const wrapper = await mountPanel();
    expect(wrapper.find('[data-testid="site-node-error"]').exists()).toBe(false);
  });

  it("disables the select while a Node install runs", async () => {
    useOperations().begin({ id: "node-install:22", kind: "tool-install", label: "Installing" });
    const wrapper = await mountPanel();
    expect(select(wrapper).attributes("disabled")).toBeDefined();
  });

  it("disables the select with a note when no release is installed", async () => {
    listNode.mockResolvedValue([]);
    const wrapper = await mountPanel();
    expect(select(wrapper).attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("No Node release is installed yet.");
  });

  it("ignores a stale response after a quick site switch", async () => {
    let resolveBlog: (r: NodeSiteResponse) => void = () => {};
    nodeSite.mockImplementation((name: string) =>
      name === "blog"
        ? new Promise<NodeSiteResponse>((resolve) => {
            resolveBlog = resolve;
          })
        : Promise.resolve(reply({ effective: "20.18.0", source: "site" })),
    );
    const wrapper = mount(SiteNodePanel, { props: { site: site("blog") } });
    await wrapper.setProps({ site: site("shop") });
    await flushPromises();
    resolveBlog(reply({ effective: "22.9.0", source: "nvmrc" }));
    await flushPromises();
    const text = wrapper.get('[data-testid="site-node-effective"]').text();
    expect(text).toContain("20.18.0");
    expect(text).toContain("from site preference");
  });
});
