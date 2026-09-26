import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import CreateLaravelWizard from "./CreateLaravelWizard.vue";
import type { ToolStatus } from "@/ipc/types";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());
const listTools = vi.hoisted(() => vi.fn());
const availablePhp = vi.hoisted(() => vi.fn());
const installPhpWithProgress = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp,
  createSite,
  installPhpWithProgress,
  installToolStreamed,
  jobCancel: vi.fn().mockResolvedValue(undefined),
  jobStatus,
  listTools,
  openInBrowser: vi.fn(),
  openPath: vi.fn(),
  pickDirectory: vi.fn(),
  pollJobToEnd,
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

function tool(id: string, overrides: Partial<ToolStatus> = {}): ToolStatus {
  return { id, display_name: id, installed: true, version: "1.0", binaries: [id], ...overrides };
}

const ALL_TOOLS = [tool("composer"), tool("laravel"), tool("node")];

function status(state: string, error: string | null = null) {
  return { log: [], next_cursor: 0, phase: "Scaffolding", state, error };
}

async function openWizard(phpVersions = ["8.3", "8.4"], defaultPhp = "8.4") {
  const w = mount(CreateLaravelWizard, {
    props: {
      open: false,
      parkedFolders: ["/srv"],
      phpVersions,
      defaultPhp,
      tld: "test",
      report: null,
    },
    global: { stubs: { teleport: true } },
  });
  await w.setProps({ open: true });
  await flushPromises();
  return w;
}

function byText(w: Awaited<ReturnType<typeof openWizard>>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

async function next(w: Awaited<ReturnType<typeof openWizard>>) {
  await byText(w, "Next")!.trigger("click");
}

async function toReview(w: Awaited<ReturnType<typeof openWizard>>, name = "blog") {
  await w.find("#cs-name").setValue(name);
  await next(w);
  await next(w);
  await next(w);
}

function submittedOptions() {
  return createSite.mock.calls[0][0].framework.options;
}

describe("CreateLaravelWizard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listTools.mockResolvedValue(ALL_TOOLS);
    createSite.mockResolvedValue("job-1");
    jobStatus.mockResolvedValue(status("succeeded"));
  });

  it("is titled for Laravel", async () => {
    const w = await openWizard();
    expect(w.text()).toContain("Create a new Laravel site");
  });

  it("submits the default spec on the default PHP", async () => {
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    expect(createSite).toHaveBeenCalledWith({
      name: "blog",
      parent_dir: "/srv",
      php: "8.4",
      secure: false,
      framework: {
        framework: "laravel",
        options: {
          starter_kit: "none",
          auth: "laravel",
          livewire_class_components: false,
          teams: false,
          testing: "pest",
          database: "sqlite",
          js: "skip",
          git: true,
          boost: false,
        },
      },
    });
    expect(w.emitted("created")).toBeTruthy();
  });

  it("honours a supported default PHP that isn't the newest", async () => {
    const w = await openWizard(["8.3", "8.4"], "8.3");
    expect((w.find("#cs-php").element as HTMLSelectElement).value).toBe("8.3");
  });

  it("defaults to the newest supported PHP when the default is outside 8.3-8.5", async () => {
    const w = await openWizard(["8.2", "8.3", "8.4"], "8.2");
    expect((w.find("#cs-php").element as HTMLSelectElement).value).toBe("8.4");
    expect(w.find("#cs-php").text()).not.toContain("8.2");
  });

  it("replaces a selection that stops being installed", async () => {
    const w = await openWizard(["8.3", "8.4"], "8.3");
    await w.setProps({ phpVersions: ["8.4"] });
    expect((w.find("#cs-php").element as HTMLSelectElement).value).toBe("8.4");
  });

  it("gates Next on a valid name", async () => {
    const w = await openWizard();
    await w.find("#cs-name").setValue("-bad");
    expect(byText(w, "Next")!.attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("Use a single label");
    await w.find("#cs-name").setValue("good");
    expect(byText(w, "Next")!.attributes("disabled")).toBeUndefined();
  });

  it("sends the chosen JS runtime for an Inertia kit and skip for Livewire", async () => {
    const w = await openWizard();
    await w.find("#cs-name").setValue("blog");
    await next(w);
    await byText(w, "Vue")!.trigger("click");
    await w.find("#cs-js").setValue("bun");
    await next(w);
    await next(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    expect(submittedOptions()).toMatchObject({ starter_kit: "vue", js: "bun" });

    createSite.mockClear();
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    await w.find("#cs-name").setValue("blog");
    await next(w);
    await byText(w, "Livewire")!.trigger("click");
    await next(w);
    await next(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    expect(submittedOptions()).toMatchObject({ starter_kit: "livewire", js: "skip" });
  });

  it("requires a community package before leaving the Stack step", async () => {
    const w = await openWizard();
    await w.find("#cs-name").setValue("blog");
    await next(w);
    await byText(w, "Community")!.trigger("click");
    expect(byText(w, "Next")!.attributes("disabled")).toBeDefined();
    await w.find("#cs-pkg").setValue("acme/kit");
    await next(w);
    await next(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    expect(submittedOptions().starter_kit).toEqual({ community: "acme/kit" });
  });

  it("notes that a missing Node is installed during creation", async () => {
    listTools.mockResolvedValue([tool("composer"), tool("laravel")]);
    const w = await openWizard();
    await w.find("#cs-name").setValue("blog");
    await next(w);
    await byText(w, "React")!.trigger("click");
    expect(w.text()).toContain("Node will be installed automatically.");
    await next(w);
    await next(w);
    expect(w.text()).toContain("Node will be installed automatically during creation.");
  });

  it("blocks the installer row until Yerd's own Composer is present", async () => {
    listTools.mockResolvedValue([tool("composer", { installed: false, external: true })]);
    const w = await openWizard();

    expect(w.text()).toContain("A few tools are needed first");
    const installs = w.findAll("button").filter((b) => b.text() === "Install");
    const install = installs[installs.length - 1];
    expect(install.attributes("disabled")).toBeDefined();
    expect(install.attributes("title")).toContain("Composer is required");

    await byText(w, "Install missing tools")!.trigger("click");
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith(
      "Can't install the Laravel installer",
      expect.any(String),
    );
    expect(installToolStreamed).not.toHaveBeenCalled();
  });

  it("installs missing tools in dependency order", async () => {
    listTools
      .mockResolvedValueOnce([])
      .mockResolvedValue([tool("composer")]);
    availablePhp.mockResolvedValue({ available: ["8.2", "8.4", "8.6"] });
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockResolvedValue({ state: "succeeded", error: null });
    const w = await openWizard([], "");

    await byText(w, "Install missing tools")!.trigger("click");
    await flushPromises();

    expect(installPhpWithProgress).toHaveBeenCalledWith("8.4", expect.any(Function));
    expect(installToolStreamed.mock.calls.map((c) => c[0])).toEqual(["composer", "laravel"]);
    expect(toastSuccess).toHaveBeenCalledWith("Toolchain ready");
  });

  it("returns to Review with the form kept after a failed create", async () => {
    jobStatus.mockResolvedValue(status("failed", "composer exploded"));
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    expect(w.text()).toContain("composer exploded");
    await byText(w, "Back")!.trigger("click");
    expect(w.text()).toContain("blog.test");
    expect(byText(w, "Create site")).toBeTruthy();
  });

  it("shows the lowercased folder the daemon will create", async () => {
    const w = await openWizard();
    await w.find("#cs-name").setValue("Blog");
    expect(w.text()).toContain("/srv/blog");
    expect(w.text()).not.toContain("/srv/Blog");
  });

  it("keeps polling while closed, announces the result and resumes on reopen", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValue(status("succeeded"));
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    await w.setProps({ open: false });
    await w.setProps({ open: true });
    expect(w.find("#cs-name").exists()).toBe(false);
    expect(w.text()).toContain("Scaffolding");

    await w.setProps({ open: false });
    await vi.waitFor(() => expect(w.emitted("created")).toBeTruthy(), { timeout: 3000 });
    expect(toastSuccess).toHaveBeenCalledWith("blog.test is ready");
    w.unmount();
  });

  it("announces a status failure that happens while closed", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockRejectedValue(new Error("daemon restarted"));
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    await w.setProps({ open: false });
    await vi.waitFor(
      () => expect(toastError).toHaveBeenCalledWith("Couldn't create blog.test", "daemon restarted"),
      { timeout: 2000 },
    );
    w.unmount();
  });

  it("re-enables Cancel when a cancelled create is retried", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("cancelled"))
      .mockResolvedValue(status("running"));
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    await byText(w, "Cancel")!.trigger("click");

    await vi.waitFor(() => expect(byText(w, "Back")).toBeTruthy(), { timeout: 2000 });
    await byText(w, "Back")!.trigger("click");
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();

    const cancel = byText(w, "Cancel")!;
    expect(cancel.text()).toBe("Cancel");
    expect(cancel.attributes("disabled")).toBeUndefined();
    w.unmount();
  });

  it("resets the form when reopened after a finished create", async () => {
    const w = await openWizard();
    await toReview(w);
    await byText(w, "Create site")!.trigger("click");
    await flushPromises();
    expect(w.text()).toContain("blog.test is ready");

    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect((w.find("#cs-name").element as HTMLInputElement).value).toBe("");
  });
});
