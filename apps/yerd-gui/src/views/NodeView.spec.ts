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
import { useToast } from "@/composables/useToast";
import type { MajorLine, NodeRelease, NodeSiteEntry, StatusReport } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };

/** reka-ui only mounts dropdown content after pointer events jsdom does not
 *  deliver, so the row menu renders inline with each item as a plain button. */
const dropdownStubs = {
  DropdownMenu: Pass,
  DropdownMenuTrigger: Pass,
  DropdownMenuContent: Pass,
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

const current: NodeRelease = {
  version: "24.2.0",
  lts: false,
  default: false,
  uninstall_blocked: false,
};

const gallium: NodeRelease = {
  version: "16.20.2",
  lts: true,
  codename: "Gallium",
  default: false,
  uninstall_blocked: false,
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

const sites: NodeSiteEntry[] = [
  { site: "home", effective: "22.9.0", source: "default" },
  { site: "shop", effective: "22.9.0", source: "nvmrc" },
  { site: "codestash", effective: "20.18.0", source: "nvmrc" },
  { site: "blog", effective: "20.18.0", source: "site", preference: "20" },
  {
    site: "legacy",
    project_file: { kind: "nvmrc", selector: "18" },
    missing: "18",
    error: "Node 18 is not installed",
  },
];

function stubIpc(
  opts: {
    installed?: NodeRelease[];
    majors?: MajorLine[] | Error;
    sites?: NodeSiteEntry[] | Error;
    jobLog?: string[];
  } = {},
) {
  const installed = opts.installed ?? [jod, iron];
  const majors = opts.majors ?? lines;
  const siteEntries = opts.sites ?? sites;
  let jobPolls = 0;
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_node":
        return Promise.resolve({ type: "node_versions", installed });
      case "available_node":
        return majors instanceof Error
          ? Promise.reject(majors)
          : Promise.resolve({ type: "available_node", majors });
      case "node_sites":
        return siteEntries instanceof Error
          ? Promise.reject(siteEntries)
          : Promise.resolve({ type: "node_sites", sites: siteEntries });
      case "set_node_version":
        return Promise.resolve({ type: "ok" });
      case "uninstall_node":
        return Promise.resolve({ type: "node_versions", installed: [jod] });
      case "install_node_streamed":
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      case "job_status":
        jobPolls += 1;
        if (opts.jobLog) {
          return Promise.resolve({
            type: "job_progress",
            state: jobPolls === 1 ? "running" : "succeeded",
            phase: "",
            log: jobPolls === 1 ? opts.jobLog : [],
            next_cursor: jobPolls,
            error: null,
          });
        }
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
    attachTo: document.body,
    global: {
      stubs: {
        teleport: true,
        RouterLink: true,
        DialogPortal: Pass,
        AlertDialogPortal: Pass,
        ...dropdownStubs,
      },
    },
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

function confirmButton(wrapper: VueWrapper, label: string) {
  const b = wrapper
    .get('[role="alertdialog"]')
    .findAll("button")
    .find((x) => x.text().trim() === label);
  if (!b) throw new Error(`${label} button not rendered in the confirm`);
  return b;
}

function callsOf(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function usedBy(wrapper: VueWrapper, version: string) {
  return wrapper.get(`[data-testid="node-row-${version}"] [data-testid="node-used-by"]`).text();
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

describe("NodeView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetResourceCache();
    useDaemon().report.value = null;
    for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
    document.body.innerHTML = "";
    const ops = useOperations();
    ops.active.value.forEach((o) => ops.end(o.id));
  });

  it("renders codename, default marker, update chip and the bare-shim note", async () => {
    stubIpc();
    const wrapper = await mountView();
    const row = wrapper.get('[data-testid="node-row-22.9.0"]');
    expect(row.text()).toContain("LTS Jod");
    expect(row.find('[data-testid="node-default-badge"]').exists()).toBe(true);
    expect(row.text()).toContain("↑ 22.9.0 → 22.12.0");
    expect(
      wrapper.get('[data-testid="node-row-20.18.0"]').find('[data-testid="node-default-badge"]')
        .exists(),
    ).toBe(false);
    expect(wrapper.get('[data-testid="node-shim-note"]').text()).toContain("yerd exec npm");
  });

  it("counts the sites using each release", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(callsOf("node_sites")).toHaveLength(1);
    expect(usedBy(wrapper, "22.9.0")).toBe("2 sites");
    expect(usedBy(wrapper, "20.18.0")).toBe("2 sites");
  });

  it("shows a dash for a release no site uses", async () => {
    stubIpc({ installed: [jod, iron, current], sites: sites.slice(0, 3) });
    const wrapper = await mountView();
    expect(usedBy(wrapper, "22.9.0")).toBe("2 sites");
    expect(usedBy(wrapper, "20.18.0")).toBe("1 site");
    expect(usedBy(wrapper, "24.2.0")).toBe("—");
  });

  it("lists the sites off the default with their source and chip", async () => {
    stubIpc();
    useDaemon().report.value = { tld: "test" } as unknown as StatusReport;
    const wrapper = await mountView();
    const rows = wrapper.findAll('[data-testid="node-off-default-row"]').map((r) => r.text());
    expect(rows).toHaveLength(3);
    expect(rows[0]).toContain("codestash.test");
    expect(rows[0]).toContain(".nvmrc");
    expect(rows[0]).toContain("Node 20");
    expect(rows[1]).toContain("blog.test");
    expect(rows[1]).toContain("saved");
    expect(rows[2]).toContain("legacy.test");
    expect(rows[2]).toContain("Node 18 missing");
    expect(wrapper.get('[data-testid="node-off-default"]').text()).not.toContain("home.test");
    expect(wrapper.get('[data-testid="node-off-default"]').text()).not.toContain("shop.test");
  });

  it("says every site uses the default when none is off it", async () => {
    stubIpc({ sites: sites.slice(0, 2) });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-off-default-empty"]').exists()).toBe(true);
  });

  it("names the installed default in the pick-order card", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(wrapper.get('[data-testid="node-pick-order"]').text()).toContain("Node 22.9.0");
  });

  it("keeps the table when the site sweep fails", async () => {
    stubIpc({ sites: new Error("sweep failed") });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-row-22.9.0"]').exists()).toBe(true);
    expect(usedBy(wrapper, "22.9.0")).toBe("—");
    expect(wrapper.find('[data-testid="node-off-default-error"]').exists()).toBe(true);
  });

  it("adds Current and EOL chips once the release index has loaded", async () => {
    stubIpc({ installed: [jod, current, gallium] });
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="node-tag-current"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="node-tag-eol"]').exists()).toBe(false);
    await openInstall(wrapper);
    const tagsOf = (v: string) => wrapper.get(`[data-testid="node-row-${v}"]`);
    expect(tagsOf("24.2.0").find('[data-testid="node-tag-current"]').exists()).toBe(true);
    expect(tagsOf("16.20.2").find('[data-testid="node-tag-eol"]').exists()).toBe(true);
    expect(tagsOf("22.9.0").find('[data-testid="node-tag-current"]').exists()).toBe(false);
    expect(tagsOf("22.9.0").find('[data-testid="node-tag-eol"]').exists()).toBe(false);
  });

  it("sets the global default with a null site and refetches the site sweep", async () => {
    stubIpc();
    const wrapper = await mountView();
    expect(
      buttonIn(wrapper, "node-row-22.9.0", "Make default").attributes("disabled"),
    ).toBeDefined();
    await buttonIn(wrapper, "node-row-20.18.0", "Make default").trigger("click");
    await flushPromises();
    expect(callsOf("set_node_version")[0]?.[1]).toEqual({ version: "20.18.0", site: null });
    expect(callsOf("node_sites")).toHaveLength(2);
  });

  it("disables uninstall when the daemon reports it blocked", async () => {
    stubIpc();
    const wrapper = await mountView();
    const blocked = buttonIn(wrapper, "node-row-22.9.0", "Uninstall…");
    expect(blocked.attributes("disabled")).toBeDefined();
    expect(blocked.attributes("title")).toContain("set another version as default first");
    expect(
      buttonIn(wrapper, "node-row-20.18.0", "Uninstall…").attributes("disabled"),
    ).toBeUndefined();
  });

  it("lists cleared preferences by domain or root, then uninstalls", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-20.18.0", "Uninstall…").trigger("click");
    await flushPromises();
    const body = wrapper.get('[data-testid="node-uninstall-body"]').text();
    expect(body).toContain("Saved preference cleared for: blog.test, /srv/old-app");
    expect(body).not.toContain("leave your PATH");
    await confirmButton(wrapper, "Uninstall").trigger("click");
    await flushPromises();
    expect(callsOf("uninstall_node")[0]?.[1]).toEqual({ version: "20.18.0" });
    expect(wrapper.find('[data-testid="node-row-20.18.0"]').exists()).toBe(false);
    expect(callsOf("node_sites")).toHaveLength(2);
  });

  it("calls nothing when the uninstall confirm is cancelled", async () => {
    stubIpc();
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-20.18.0", "Uninstall…").trigger("click");
    await flushPromises();
    await confirmButton(wrapper, "Cancel").trigger("click");
    await flushPromises();
    expect(wrapper.find('[role="alertdialog"]').exists()).toBe(false);
    expect(callsOf("uninstall_node")).toHaveLength(0);
  });

  it("warns that the shims leave PATH when uninstalling the only release", async () => {
    stubIpc({ installed: [{ ...iron, clears_preferences: undefined }] });
    const wrapper = await mountView();
    await buttonIn(wrapper, "node-row-20.18.0", "Uninstall…").trigger("click");
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
    expect(wrapper.find('[data-testid="node-install-body"]').exists()).toBe(false);
    expect(callsOf("node_sites")).toHaveLength(2);
  });

  it("streams the install log and keeps installing after Hide", async () => {
    stubIpc({ jobLog: ["Downloading node-v24.2.0", "Extracting"] });
    const wrapper = await mountView();
    await openInstall(wrapper);
    await wrapper.get('[data-testid="node-install-submit"]').trigger("click");
    await flushPromises();
    const log = wrapper.get('[data-testid="node-install-log"]');
    expect(log.text()).toContain("Downloading node-v24.2.0");
    expect(log.text()).toContain("Extracting");
    expect(wrapper.text()).toContain("Installing Node 24");
    const hide = wrapper.findAll("button").find((b) => b.text().trim() === "Hide");
    await hide!.trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="node-install-log"]').exists()).toBe(false);
    await vi.waitFor(() => expect(toastTitles()).toContain("Installed Node 24"), {
      timeout: 2000,
    });
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
    await buttonIn(wrapper, "node-row-22.9.0", "Update to 22.12.0").trigger("click");
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
    expect(
      buttonIn(wrapper, "node-row-20.18.0", "Make default").attributes("disabled"),
    ).toBeDefined();
    expect(
      buttonIn(wrapper, "node-row-20.18.0", "Uninstall…").attributes("disabled"),
    ).toBeDefined();
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
    await wrapper
      .findAll("button")
      .find((b) => b.text().trim() === "Cancel")!
      .trigger("click");
    await flushPromises();
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
    expect(callsOf("node_sites")).toHaveLength(1);
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
    await wrapper
      .findAll("button")
      .find((b) => b.text().trim() === "Cancel")!
      .trigger("click");
    await flushPromises();
    let finish: (v: unknown) => void = () => {};
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "available_node"
        ? new Promise((resolve) => (finish = resolve))
        : Promise.resolve({ type: "ok" }),
    );
    await openInstall(wrapper);
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
