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

  it("renders release badges and Install, Update and Installed states", async () => {
    stubIpc();
    const wrapper = await mountView();
    const r16 = wrapper.get('[data-testid="node-major-16"]').text();
    expect(r16).toContain("LTS Gallium");
    expect(r16).toContain("EOL");
    expect(r16).toContain("Install");
    expect(wrapper.get('[data-testid="node-major-22"]').text()).toContain("Update");
    expect(wrapper.get('[data-testid="node-major-20"]').text()).toContain("Installed");
    expect(wrapper.get('[data-testid="node-major-24"]').text()).not.toContain("EOL");
    expect(wrapper.find('[data-testid="node-eol-unknown"]').exists()).toBe(false);
  });

  it("installs a major's newest patch without moving the default", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-major-24", "Install").trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({ version: "24", update: false });
  });

  it("sends update: true for an Update", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-major-22", "Update").trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({ version: "22", update: true });
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
    const submit = wrapper.get('[data-testid="node-exact-install"]');
    expect(submit.attributes("disabled")).toBeDefined();
    await wrapper.get('[aria-label="Exact Node version"]').setValue(" 22.9.0 ");
    await submit.trigger("click");
    await flushPromises();
    expect(callsOf("install_node_streamed")[0]?.[1]).toEqual({
      version: "22.9.0",
      update: false,
    });
  });

  it("disables every mutating action while a node install runs", async () => {
    stubIpc();
    useOperations().begin({ id: "node-install:24", kind: "tool-install", label: "Installing" });
    const wrapper = await mountView();
    expect(buttonIn(wrapper, "node-major-16", "Install").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-major-22", "Update").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-row-20.18.0", "Set default").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "node-row-20.18.0", "Uninstall").attributes("disabled")).toBeDefined();
    await wrapper.get('[aria-label="Exact Node version"]').setValue("22.9.0");
    expect(wrapper.get('[data-testid="node-exact-install"]').attributes("disabled")).toBeDefined();
  });

  it("keeps the installed table when the release list fails", async () => {
    stubIpc({ majors: new Error("offline") });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-row-22.9.0"]').exists()).toBe(true);
    const err = wrapper.get('[data-testid="node-releases-error"]');
    expect(err.text()).toContain("offline");
    expect(err.text()).toContain("Retry");
  });

  it("shows the error and Retry when a refresh fails over cached releases", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-releases-error"]').exists()).toBe(false);
    stubIpc({ majors: new Error("index unreachable") });
    const refresh = wrapper.findAll("button").find((b) => b.text() === "Refresh");
    if (!refresh) throw new Error("Refresh button not rendered");
    await refresh.trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="node-releases-error"]').text()).toContain(
      "index unreachable",
    );
    expect(wrapper.find('[data-testid="node-major-22"]').exists()).toBe(true);
    stubIpc();
    await buttonIn(wrapper, "node-releases-error", "Retry").trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="node-releases-error"]').exists()).toBe(false);
  });

  it("notes when end-of-life status is unavailable", async () => {
    stubIpc({ majors: lines.map(({ eol: _eol, ...rest }) => rest) });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-eol-unknown"]').exists()).toBe(true);
    expect(wrapper.get('[data-testid="node-major-16"]').text()).not.toContain("EOL");
  });

  it("shows the empty state with no release installed", async () => {
    stubIpc({ installed: [] });
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("No Node versions installed yet");
  });
});
