import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import NodeView from "./NodeView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { resetResourceCache } from "@/composables/useResource";
import type { MajorLine, NodeRelease } from "@/ipc/types";

/** reka-ui only mounts dropdown content after pointer events jsdom does not
 *  deliver, so the row menu renders inline with each item as a plain button. */
const dropdownStubs = {
  DropdownMenu: { template: "<div><slot /></div>" },
  DropdownMenuTrigger: { template: "<div><slot /></div>" },
  DropdownMenuContent: { template: "<div><slot /></div>" },
  DropdownMenuItem: {
    props: ["disabled"],
    emits: ["select"],
    template: `<button type="button" :disabled="disabled" @click="$emit('select')"><slot /></button>`,
  },
  DropdownMenuSeparator: { template: "<hr />" },
};

const jod: NodeRelease = {
  version: "22.9.0",
  lts: true,
  codename: "Jod",
  default: true,
  uninstall_blocked: true,
  update: "22.12.0",
};

const iron: NodeRelease = {
  version: "20.18.0",
  lts: true,
  default: false,
  uninstall_blocked: false,
  clears_preferences: [{ root: "/srv/blog", site: "blog" }, { root: "/srv/old-app" }],
};

const lines: MajorLine[] = [
  { major: 24, latest: "24.2.0", eol: false, update_available: false },
  {
    major: 22,
    latest: "22.12.0",
    codename: "Jod",
    eol: false,
    installed_latest: "22.9.0",
    update_available: true,
  },
  {
    major: 20,
    latest: "20.18.0",
    codename: "Iron",
    eol: false,
    installed_latest: "20.18.0",
    update_available: false,
  },
  { major: 16, latest: "16.20.2", codename: "Gallium", eol: true, update_available: false },
];

function stubIpc(
  opts: {
    installed?: NodeRelease[];
    majors?: MajorLine[] | Error;
  } = {},
) {
  const installed = opts.installed ?? [jod, iron];
  const majors = opts.majors ?? lines;
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_node":
        return Promise.resolve({ type: "node_versions", installed });
      case "available_node":
        return majors instanceof Error
          ? Promise.reject(majors)
          : Promise.resolve({ type: "available_node", majors });
      case "set_node_version":
        return Promise.resolve({ type: "ok" });
      case "uninstall_node":
        return Promise.resolve({ type: "node_versions", installed: [jod] });
      case "install_node_streamed":
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      case "job_status":
        return Promise.resolve({
          type: "job_progress",
          state: "succeeded",
          phase: "",
          log: ["Installed Node"],
          next_cursor: 1,
          error: null,
        });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(NodeView, {
    global: { stubs: { teleport: true, RouterLink: true, ...dropdownStubs } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function buttonIn(wrapper: VueWrapper, testId: string, label: string) {
  const b = wrapper
    .get(`[data-testid="${testId}"]`)
    .findAll("button")
    .find((x) => x.text().includes(label));
  if (!b) throw new Error(`${label} button not rendered in ${testId}`);
  return b;
}

function callsOf(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

describe("NodeView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
    const ops = useOperations();
    ops.active.value.forEach((o) => ops.end(o.id));
  });

  it("renders codename, default marker and the bare-shim note", async () => {
    stubIpc();
    const wrapper = await mountView();
    const row = wrapper.get('[data-testid="node-row-22.9.0"]');
    expect(row.text()).toContain("LTS Jod");
    expect(row.find('[data-testid="node-default-badge"]').exists()).toBe(true);
    expect(row.text()).toContain("22.9.0 → 22.12.0");
    expect(
      wrapper.get('[data-testid="node-row-20.18.0"]').find('[data-testid="node-default-badge"]')
        .exists(),
    ).toBe(false);
    expect(wrapper.get('[data-testid="node-shim-note"]').text()).toContain("yerd exec npm");
  });

  it("sets the global default with a null site", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(buttonIn(wrapper, "node-row-22.9.0", "Set default").attributes("disabled")).toBeDefined();
    await buttonIn(wrapper, "node-row-20.18.0", "Set default").trigger("click");
    await flushPromises();
    expect(callsOf("set_node_version")[0]?.[1]).toEqual({ version: "20.18.0", site: null });
  });

  it("disables uninstall when the daemon reports it blocked", async () => {
    stubIpc();
    const wrapper = await mountView();
    const blocked = buttonIn(wrapper, "node-row-22.9.0", "Uninstall");
    expect(blocked.attributes("disabled")).toBeDefined();
    expect(blocked.attributes("title")).toContain("set another version as default first");
    expect(
      buttonIn(wrapper, "node-row-20.18.0", "Uninstall").attributes("disabled"),
    ).toBeUndefined();
  });

  it("lists cleared preferences by site name or root, then uninstalls", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-20.18.0", "Uninstall").trigger("click");
    await flushPromises();
    const body = wrapper.get('[data-testid="node-uninstall-body"]').text();
    expect(body).toContain("Saved preference cleared for: blog, /srv/old-app");
    expect(body).not.toContain("leave your PATH");
    await wrapper.get('[data-testid="node-uninstall-confirm"]').trigger("click");
    await flushPromises();
    expect(callsOf("uninstall_node")[0]?.[1]).toEqual({ version: "20.18.0" });
    expect(wrapper.find('[data-testid="node-row-20.18.0"]').exists()).toBe(false);
  });

  it("warns that the shims leave PATH when uninstalling the only release", async () => {
    stubIpc({ installed: [{ ...iron, clears_preferences: undefined }] });
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-20.18.0", "Uninstall").trigger("click");
    await flushPromises();
    const body = wrapper.get('[data-testid="node-uninstall-body"]').text();
    expect(body).toContain("leave your PATH");
    expect(body).not.toContain("Saved preference cleared");
  });

  async function openInstall(wrapper: VueWrapper) {
    await wrapper.get('[data-testid="node-install-open"]').trigger("click");
    await flushPromises();
  }

  function optionLabels(wrapper: VueWrapper) {
    return wrapper
      .get('[aria-label="Node release to install"]')
      .findAll("option")
      .map((o) => o.text());
  }

  it("offers only supported lines that are not installed, EOL behind a toggle", async () => {
    stubIpc();
    const wrapper = await mountView();
    await openInstall(wrapper);
    expect(optionLabels(wrapper)).toEqual(["Node 24 (24.2.0)"]);
    expect(wrapper.find('[data-testid="node-eol-unknown"]').exists()).toBe(false);
    await wrapper.get('[data-testid="node-eol-toggle"]').trigger("click");
    await flushPromises();
    expect(optionLabels(wrapper)).toEqual([
      "Node 24 (24.2.0)",
      "Node 16 (16.20.2) - LTS Gallium - EOL",
    ]);
  });

  it("hides the EOL toggle when no end-of-life line is on offer", async () => {
    stubIpc({ majors: lines.filter((l) => l.eol !== true) });
    const wrapper = await mountView();
    await openInstall(wrapper);
    expect(wrapper.find('[data-testid="node-eol-toggle"]').exists()).toBe(false);
  });

  it("preselects the newest supported LTS line not yet installed", async () => {
    stubIpc({
      majors: [
        { major: 25, latest: "25.1.0", eol: false, update_available: false },
        { major: 24, latest: "24.2.0", codename: "Krypton", eol: false, update_available: false },
      ],
    });
    const wrapper = await mountView();
    await openInstall(wrapper);
    const select = wrapper.get('[aria-label="Node release to install"]')
      .element as HTMLSelectElement;
    expect(select.value).toBe("24");
  });

  it("installs the picked line's newest patch without moving the default", async () => {
    stubIpc();
    const wrapper = await mountView();
    await openInstall(wrapper);
    await wrapper.get('[data-testid="node-eol-toggle"]').trigger("click");
    await flushPromises();
    await wrapper.get('[aria-label="Node release to install"]').setValue("16");
    await wrapper.get('[data-testid="node-install-submit"]').trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({ version: "16", update: false });
  });

  it("disables Update on a release without a cached update", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(buttonIn(wrapper, "node-row-20.18.0", "Update").attributes("disabled")).toBeDefined();
    expect(wrapper.get('[data-testid="node-row-20.18.0"]').text()).not.toContain("→");
  });

  it("sends update: true from the installed row's Update action", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-22.9.0", "Update").trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({ version: "22", update: true });
  });

  it("installs the typed exact version", async () => {
    stubIpc();
    const wrapper = await mountView();
    await openInstall(wrapper);
    await wrapper.get('[data-testid="node-exact-toggle"]').trigger("click");
    await flushPromises();
    const submit = () => wrapper.get('[data-testid="node-install-submit"]');
    expect(submit().attributes("disabled")).toBeDefined();
    await wrapper.get('[aria-label="Exact Node version"]').setValue(" 22.9.0 ");
    expect(submit().attributes("disabled")).toBeUndefined();
    await submit().trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({
      version: "22.9.0",
      update: false,
    });
  });

  it("starts in exact mode when every listed line is installed", async () => {
    stubIpc({ majors: lines.filter((l) => l.installed_latest) });
    const wrapper = await mountView();
    await openInstall(wrapper);
    expect(wrapper.find('[aria-label="Exact Node version"]').exists()).toBe(true);
  });

  it("disables every mutating action while a node install runs", async () => {
    stubIpc();
    useOperations().begin({ id: "node-install:24", kind: "tool-install", label: "Installing" });
    const wrapper = await mountView();
    expect(wrapper.get('[data-testid="node-install-open"]').attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-row-22.9.0", "Update").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-row-20.18.0", "Set default").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-row-20.18.0", "Uninstall").attributes("disabled")).toBeDefined();
  });

  it("keeps the installed table and shows the error in the dialog when the list fails", async () => {
    stubIpc({ majors: new Error("offline") });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-row-22.9.0"]').exists()).toBe(true);
    await openInstall(wrapper);
    const err = wrapper.get('[data-testid="node-releases-error"]');
    expect(err.text()).toContain("offline");
    expect(err.text()).toContain("Retry");
  });

  it("shows the error and Retry when a refresh fails over cached releases", async () => {
    stubIpc();
    const wrapper = await mountView();
    await openInstall(wrapper);
    stubIpc({ majors: new Error("index unreachable") });
    await openInstall(wrapper);
    expect(wrapper.get('[data-testid="node-releases-error"]').text()).toContain(
      "index unreachable",
    );
    expect(optionLabels(wrapper)).toContain("Node 24 (24.2.0)");
    stubIpc();
    await buttonIn(wrapper, "node-releases-error", "Retry").trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="node-releases-error"]').exists()).toBe(false);
  });

  it("fetches the release list only when the dialog opens", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(callsOf("available_node")).toHaveLength(0);
    await openInstall(wrapper);
    expect(callsOf("available_node")).toHaveLength(1);
  });

  it("keeps a line picked while the release list is still refreshing", async () => {
    const two: MajorLine[] = [
      { major: 25, latest: "25.1.0", eol: false, update_available: false },
      { major: 24, latest: "24.2.0", codename: "Krypton", eol: false, update_available: false },
    ];
    stubIpc({ majors: two });
    const wrapper = await mountView();
    await openInstall(wrapper);
    let finish: (v: unknown) => void = () => {};
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "available_node"
        ? new Promise((resolve) => (finish = resolve))
        : Promise.resolve({ type: "ok" }),
    );
    await wrapper.get('[data-testid="node-install-open"]').trigger("click");
    await flushPromises();
    await wrapper.get('[aria-label="Node release to install"]').setValue("25");
    finish({ type: "available_node", majors: two });
    await flushPromises();
    const select = wrapper.get('[aria-label="Node release to install"]')
      .element as HTMLSelectElement;
    expect(select.value).toBe("25");
    stubIpc({ majors: two });
    await wrapper.get('[data-testid="node-install-submit"]').trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({ version: "25", update: false });
  });

  it("selects a line once Retry loads the list after a failed first load", async () => {
    stubIpc({ majors: new Error("offline") });
    const wrapper = await mountView();
    await openInstall(wrapper);
    stubIpc();
    await buttonIn(wrapper, "node-releases-error", "Retry").trigger("click");
    await flushPromises();
    expect(
      wrapper.get('[data-testid="node-install-submit"]').attributes("disabled"),
    ).toBeUndefined();
  });

  it("notes when end-of-life status is unavailable", async () => {
    stubIpc({ majors: lines.map(({ eol: _eol, ...rest }) => rest) });
    const wrapper = await mountView();
    await openInstall(wrapper);
    expect(wrapper.find('[data-testid="node-eol-unknown"]').exists()).toBe(true);
    expect(optionLabels(wrapper)).toContain("Node 16 (16.20.2) - LTS Gallium");
  });

  it("shows the empty state with no release installed", async () => {
    stubIpc({ installed: [] });
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("No Node versions installed yet");
  });
});
