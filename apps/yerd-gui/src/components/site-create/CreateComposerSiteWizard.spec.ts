import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import CreateComposerSiteWizard from "./CreateComposerSiteWizard.vue";
import type { ComposerFramework, ToolStatus } from "@/ipc/types";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());
const listTools = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp: vi.fn(),
  createSite,
  installPhpWithProgress: vi.fn(),
  installToolStreamed: vi.fn(),
  jobCancel: vi.fn().mockResolvedValue(undefined),
  jobStatus,
  listTools,
  openInBrowser: vi.fn(),
  openPath: vi.fn(),
  pickDirectory: vi.fn(),
  pollJobToEnd: vi.fn(),
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

function composer(overrides: Partial<ToolStatus> = {}): ToolStatus {
  return {
    id: "composer",
    display_name: "Composer",
    installed: true,
    version: "2.8.0",
    binaries: ["composer"],
    ...overrides,
  };
}

async function mountWizard(framework: ComposerFramework, phpVersions: string[]) {
  const w = mount(CreateComposerSiteWizard, {
    props: {
      open: true,
      framework,
      parkedFolders: ["/srv"],
      phpVersions,
      defaultPhp: "",
      tld: "test",
      report: null,
    },
    global: { stubs: { teleport: true } },
  });
  await flushPromises();
  return w;
}

function byText(w: Awaited<ReturnType<typeof mountWizard>>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

describe("CreateComposerSiteWizard", () => {
  beforeEach(() => {
    createSite.mockReset();
    jobStatus.mockReset();
    listTools.mockReset();
    listTools.mockResolvedValue([composer()]);
    toastSuccess.mockReset();
    toastError.mockReset();
  });

  it.each([
    ["codeigniter", "CodeIgniter"],
    ["cakephp", "CakePHP"],
    ["slim", "Slim"],
  ] as const)("titles the %s wizard after the framework", async (framework, label) => {
    const w = await mountWizard(framework, ["8.4"]);
    expect(w.text()).toContain(`Create a new ${label} site`);
  });

  it("submits a spec tagged with the chosen framework on the newest supported PHP", async () => {
    createSite.mockResolvedValue("job-1");
    jobStatus.mockResolvedValue({
      log: [],
      next_cursor: 0,
      phase: "Done",
      state: "succeeded",
      error: null,
    });
    const w = await mountWizard("cakephp", ["8.3", "8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    expect(createSite).toHaveBeenCalledWith({
      name: "shop",
      parent_dir: "/srv",
      php: "8.4",
      secure: false,
      framework: { framework: "cakephp" },
    });
    expect(w.emitted("created")).toBeTruthy();
  });

  it("shows the lowercased folder the daemon will create", async () => {
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("Shop");
    await byText(w, "Next")!.trigger("click");

    expect(w.text()).toContain("/srv/shop");
    expect(w.text()).not.toContain("/srv/Shop");
  });

  it("re-enables Cancel when a cancelled create is retried", async () => {
    createSite.mockResolvedValue("job-1");
    const status = (state: string) => ({
      log: [],
      next_cursor: 0,
      phase: "Scaffolding",
      state,
      error: null,
    });
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("cancelled"))
      .mockResolvedValue(status("running"));
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    await byText(w, "Cancel")!.trigger("click");
    expect(byText(w, "Cancelling")).toBeTruthy();

    await vi.waitFor(() => expect(byText(w, "Back")).toBeTruthy(), { timeout: 2000 });
    await byText(w, "Back")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    const cancel = byText(w, "Cancel")!;
    expect(cancel.text()).toBe("Cancel");
    expect(cancel.attributes("disabled")).toBeUndefined();
    w.unmount();
  });

  it("keeps polling after the dialog closes and resumes progress on reopen", async () => {
    createSite.mockResolvedValue("job-1");
    const status = (state: string) => ({
      log: [],
      next_cursor: 0,
      phase: "Scaffolding",
      state,
      error: null,
    });
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValue(status("succeeded"));
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    await w.setProps({ open: false });
    await w.setProps({ open: true });
    expect(w.find("#ccs-name").exists()).toBe(false);
    expect(w.text()).toContain("Scaffolding");

    await w.setProps({ open: false });
    await vi.waitFor(() => expect(w.emitted("created")).toBeTruthy(), { timeout: 3000 });
    w.unmount();
  });

  it("stops polling when unmounted while a status request is in flight", async () => {
    createSite.mockResolvedValue("job-1");
    let resolveStatus: (value: unknown) => void = () => {};
    jobStatus.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveStatus = resolve;
      }),
    );
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    expect(jobStatus).toHaveBeenCalledTimes(1);

    w.unmount();
    resolveStatus({ log: [], next_cursor: 0, phase: "Scaffolding", state: "running", error: null });
    await flushPromises();
    await new Promise((resolve) => setTimeout(resolve, 700));

    expect(jobStatus).toHaveBeenCalledTimes(1);
  });

  it("resumes the progress view when reopened before the daemon returns a job id", async () => {
    let resolveCreate: (value: string) => void = () => {};
    createSite.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      }),
    );
    jobStatus.mockResolvedValue({
      log: [],
      next_cursor: 0,
      phase: "Scaffolding",
      state: "succeeded",
      error: null,
    });
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();

    expect(w.find("#ccs-name").exists()).toBe(false);
    resolveCreate("job-1");
    await flushPromises();
    expect(w.emitted("created")).toBeTruthy();
    w.unmount();
  });

  it("stops the phase spinner when the job fails", async () => {
    createSite.mockResolvedValue("job-1");
    jobStatus.mockResolvedValue({
      log: [],
      next_cursor: 0,
      phase: "Scaffolding",
      state: "failed",
      error: "composer exploded",
    });
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    expect(w.text()).toContain("composer exploded");
    expect(w.find(".animate-spin").exists()).toBe(false);
    w.unmount();
  });

  it.each([
    ["succeeded", null],
    ["failed", "composer exploded"],
  ] as const)("toasts a %s create that finishes while the dialog is closed", async (state, error) => {
    createSite.mockResolvedValue("job-1");
    jobStatus
      .mockResolvedValueOnce({ log: [], next_cursor: 0, phase: "Scaffolding", state: "running", error: null })
      .mockResolvedValue({ log: [], next_cursor: 0, phase: "Done", state, error });
    const w = await mountWizard("slim", ["8.4"]);

    await w.find("#ccs-name").setValue("shop");
    await byText(w, "Next")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    await w.setProps({ open: false });

    const toast = state === "succeeded" ? toastSuccess : toastError;
    await vi.waitFor(() => expect(toast).toHaveBeenCalledTimes(1), { timeout: 2000 });
    expect(Boolean(w.emitted("created"))).toBe(state === "succeeded");
    w.unmount();
  });

  it("gates on PHP when every installed version is below the framework minimum", async () => {
    const w = await mountWizard("codeigniter", ["7.4"]);
    expect(w.text()).toContain("A few tools are needed first");
    expect(w.text()).toContain("Installed, but not 8.1+");
  });

  it("accepts older PHP for Slim", async () => {
    const w = await mountWizard("slim", ["7.4"]);
    expect(w.text()).not.toContain("A few tools are needed first");
    expect(w.find("#ccs-name").exists()).toBe(true);
  });

  it("treats an external Composer as satisfying the prerequisite", async () => {
    listTools.mockResolvedValue([composer({ installed: false, external: true })]);
    const w = await mountWizard("slim", ["8.4"]);
    expect(w.text()).not.toContain("A few tools are needed first");
  });

  it("gates on Composer when it is neither managed nor external", async () => {
    listTools.mockResolvedValue([]);
    const w = await mountWizard("slim", ["8.4"]);
    expect(w.text()).toContain("A few tools are needed first");
    expect(byText(w, "Install missing tools")).toBeTruthy();
  });
});
