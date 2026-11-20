import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { computed, ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());
const jobCancel = vi.hoisted(() => vi.fn());
const listTools = vi.hoisted(() => vi.fn());
const listNode = vi.hoisted(() => vi.fn());
const listServices = vi.hoisted(() => vi.fn());
const availableWordPressVersions = vi.hoisted(() => vi.fn());
const availablePhp = vi.hoisted(() => vi.fn());
const installPhpWithProgress = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const openInBrowser = vi.hoisted(() => vi.fn());
const openPath = vi.hoisted(() => vi.fn());
const mintWordPressLoginToken = vi.hoisted(() => vi.fn());
const getInstalledIdes = vi.hoisted(() => vi.fn());
const getPreferredIde = vi.hoisted(() => vi.fn());
const getSiteIdeOverrides = vi.hoisted(() => vi.fn());
const openInIde = vi.hoisted(() => vi.fn());
const openInSystemDefault = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp,
  availableWordPressVersions,
  createSite,
  getInstalledIdes,
  getPreferredIde,
  getSiteIdeOverrides,
  installPhpWithProgress,
  installToolStreamed,
  jobCancel,
  jobStatus,
  listNode,
  listServices,
  listTools,
  mintWordPressLoginToken,
  openInBrowser,
  openInIde,
  openInSystemDefault,
  openPath,
  pickDirectory: vi.fn(),
  pollJobToEnd,
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

const hostPlatform = ref("macos");
vi.mock("@/composables/usePlatform", () => ({
  usePlatform: () => ({
    platform: hostPlatform,
    isMac: computed(() => hostPlatform.value === "macos"),
    supportsPathInstall: computed(() => true),
    home: ref(null),
  }),
}));

import NewSiteDialog from "./NewSiteDialog.vue";
import StarterPicker from "./StarterPicker.vue";
import { resetCreateSiteJob } from "@/composables/useCreateSiteJob";
import { resetIdes } from "@/composables/useIdes";
import { resetResourceCache } from "@/composables/useResource";
import type { NodeRelease, ToolStatus } from "@/ipc/types";

function tool(id: string, overrides: Partial<ToolStatus> = {}): ToolStatus {
  return { id, display_name: id, installed: true, version: "1.0", binaries: [id], ...overrides };
}

const ALL_TOOLS = [tool("composer"), tool("laravel"), tool("node"), tool("wp-cli")];

function status(state: string, error: string | null = null, phase = "Scaffolding") {
  return { log: [], next_cursor: 0, phase, state, error };
}

const Pass = { template: "<div><slot /></div>" };

async function openDialog(
  phpVersions = ["8.3", "8.4"],
  defaultPhp = "8.4",
  extra: Record<string, unknown> = {},
) {
  const w = mount(NewSiteDialog, {
    props: {
      open: false,
      parkedFolders: ["/srv"],
      phpVersions,
      defaultPhp,
      tld: "test",
      report: null,
      home: null,
      ...extra,
    },
    global: { stubs: { teleport: true, DialogPortal: Pass, ComboboxPortal: Pass } },
  });
  await w.setProps({ open: true });
  await flushPromises();
  return w;
}

type Dialog = Awaited<ReturnType<typeof openDialog>>;

function byText(w: Dialog, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

async function click(w: Dialog, text: string) {
  const b = byText(w, text);
  if (!b) throw new Error(`no "${text}" button`);
  await b.trigger("click");
  await flushPromises();
}

async function setName(w: Dialog, name: string) {
  await w.get("#ns-name").setValue(name);
}

async function toOptions(w: Dialog, name = "blog") {
  await setName(w, name);
  await click(w, "Next: Options");
}

function checkedPhp(w: Dialog): string | undefined {
  return w.get('[aria-label="PHP version"]').find('[aria-checked="true"]').text();
}

async function pickPhp(w: Dialog, version: string) {
  const radio = w
    .get('[aria-label="PHP version"]')
    .findAll('[role="radio"]')
    .find((r) => r.text().startsWith(version));
  await radio?.trigger("click");
}

function spec() {
  return createSite.mock.calls[0]?.[0];
}

beforeEach(() => {
  vi.clearAllMocks();
  resetCreateSiteJob();
  resetResourceCache();
  resetIdes();
  hostPlatform.value = "macos";
  listTools.mockResolvedValue(ALL_TOOLS);
  listNode.mockResolvedValue([]);
  listServices.mockResolvedValue([]);
  availableWordPressVersions.mockResolvedValue([]);
  createSite.mockResolvedValue("job-1");
  jobStatus.mockResolvedValue(status("succeeded", null, "Done"));
  jobCancel.mockResolvedValue(undefined);
  getInstalledIdes.mockResolvedValue([]);
  getPreferredIde.mockResolvedValue(null);
  getSiteIdeOverrides.mockResolvedValue({});
});

describe("NewSiteDialog Project step", () => {
  it("renders one step at a time while moving back and forth", async () => {
    const w = await openDialog();
    await toOptions(w);
    expect(w.text()).toContain("Starter kit");
    expect(w.text()).not.toContain("Start from");
    await click(w, "Back");
    await click(w, "Next: Options");
    await click(w, "Back");
    expect(w.text().split("Start from")).toHaveLength(2);
    expect(w.text()).not.toContain("Starter kit");
  });

  it("shows the lowercased folder the daemon will create", async () => {
    const w = await openDialog();
    await setName(w, "Blog");
    expect(w.get('[data-testid="ns-preview"]').text()).toContain("/srv/blog");
    expect(w.text()).not.toContain("/srv/Blog");
  });

  it("gates Next on a valid name", async () => {
    const w = await openDialog();
    await setName(w, "-bad");
    expect(byText(w, "Next: Options")?.attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("Use a single label");
    await setName(w, "good");
    expect(byText(w, "Next: Options")?.attributes("disabled")).toBeUndefined();
  });

  it.each(["blog", "Blog", " BLOG "])(
    "refuses %j when a site with that name already exists",
    async (name) => {
      const w = await openDialog(["8.4"], "8.4", { existingNames: ["shop", "blog"] });
      await setName(w, name);
      expect(byText(w, "Next: Options")?.attributes("disabled")).toBeDefined();
      expect(w.get('[data-testid="ns-name-taken"]').text()).toBe("A site named blog already exists.");
      await w.get("#ns-name").trigger("keydown", { key: "Enter" });
      await flushPromises();
      expect(w.text()).not.toContain("Starter kit");
      await setName(w, "blog2");
      expect(w.find('[data-testid="ns-name-taken"]').exists()).toBe(false);
      expect(byText(w, "Next: Options")?.attributes("disabled")).toBeUndefined();
    },
  );

  it("sends a name that became taken back to Project instead of creating it", async () => {
    const w = await openDialog();
    await toOptions(w);
    await w.setProps({ existingNames: ["blog"] });
    await click(w, "Create site");
    expect(createSite).not.toHaveBeenCalled();
    expect(w.find('[data-testid="ns-name-taken"]').exists()).toBe(true);
  });

  it("prefills the name it was opened with", async () => {
    const w = await openDialog(["8.4"], "8.4", { initialName: "invo" });
    expect((w.get("#ns-name").element as HTMLInputElement).value).toBe("invo");
  });

  it("disables Next while the toolchain is still loading", async () => {
    listTools.mockReturnValue(new Promise(() => {}));
    const w = await openDialog();
    await setName(w, "blog");
    expect(byText(w, "Next: Options")?.attributes("disabled")).toBeDefined();
  });

  it("emits existing and closes from the Existing folder tile", async () => {
    const w = await openDialog();
    await click(w, "Existing folder");
    expect(w.emitted("existing")).toHaveLength(1);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("keeps the typed basics across starters and re-picks an unsupported PHP", async () => {
    const w = await openDialog(["7.4", "8.4"], "8.4");
    await w.get('[data-starter="wordpress"]').trigger("click");
    await setName(w, "shop");
    await pickPhp(w, "7.4");
    expect(checkedPhp(w)).toBe("7.4 (legacy)");
    await w.get('[data-starter="laravel"]').trigger("click");
    await flushPromises();
    expect((w.get("#ns-name").element as HTMLInputElement).value).toBe("shop");
    expect(checkedPhp(w)).toBe("8.4");
  });

  it("keeps a legacy WordPress PHP pick through a status tick", async () => {
    const w = await openDialog(["7.4", "8.4"], "8.4", { initialStarter: "wordpress" });
    await pickPhp(w, "7.4");
    await w.setProps({ phpVersions: ["7.4", "8.4"] });
    expect(checkedPhp(w)).toBe("7.4 (legacy)");
  });

  it("preselects PHP immediately when reopened on the same starter", async () => {
    const w = await openDialog(["8.3", "8.4"], "8.3");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    expect(checkedPhp(w)).toBe("8.3");
  });

  it("derives the WordPress database name from a name typed before switching", async () => {
    const w = await openDialog();
    await setName(w, "my-blog");
    await w.get('[data-starter="wordpress"]').trigger("click");
    await click(w, "Next: Options");
    expect((w.get("#ns-wp-dbname").element as HTMLInputElement).value).toBe("my_blog");
  });

  it("derives the WordPress database name from the prefilled name", async () => {
    const w = await openDialog(["8.4"], "8.4", { initialName: "my-shop", initialStarter: "wordpress" });
    await click(w, "Next: Options");
    expect((w.get("#ns-wp-dbname").element as HTMLInputElement).value).toBe("my_shop");
  });
});

describe("NewSiteDialog missing tools", () => {
  it("goes to the tools step when Composer is missing and advances once it is installed", async () => {
    listTools.mockResolvedValueOnce([tool("laravel")]).mockResolvedValue(ALL_TOOLS);
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockResolvedValue({ state: "succeeded", error: null });
    const w = await openDialog();
    await toOptions(w);
    expect(w.text()).toContain("Laravel needs one more tool");
    await click(w, "Install missing tools");
    expect(installToolStreamed.mock.calls.map((c) => c[0])).toEqual(["composer"]);
    expect(w.text()).toContain("Starter kit");
  });

  it("marks the tool being installed and queues the rest", async () => {
    listTools.mockResolvedValue([]);
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockReturnValue(new Promise(() => {}));
    const w = await openDialog();
    await toOptions(w);
    expect(w.text()).toContain("Laravel needs two more tools");
    await byText(w, "Install missing tools")?.trigger("click");
    await flushPromises();
    expect(w.get('[data-tool="composer"]').text()).toContain("Installing");
    expect(w.get('[data-tool="laravel"]').text()).toContain("Queued");
    expect(byText(w, "Installing…")?.attributes("disabled")).toBeDefined();
  });

  it("fills an empty PHP once the tools step installs one", async () => {
    const w = await openDialog([], "");
    await toOptions(w);
    expect(w.get('[data-tool="php"]').text()).toContain("Install");
    await w.setProps({ phpVersions: ["8.4"] });
    await flushPromises();
    expect(w.text()).toContain("Starter kit");
    await click(w, "Create site");
    expect(spec()?.php).toBe("8.4");
  });
});

describe("NewSiteDialog options and create", () => {
  it("submits the default Laravel spec on the default PHP", async () => {
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
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
          js: "npm",
          git: true,
          boost: false,
        },
      },
    });
    expect(w.emitted("created")).toBeTruthy();
  });

  it("builds the Laravel spec from the drawn controls and the kept switches", async () => {
    listNode.mockResolvedValue([
      { version: "22.11.0", lts: true, default: true, uninstall_blocked: false },
      { version: "20.18.0", lts: true, default: false, uninstall_blocked: false },
    ] satisfies NodeRelease[]);
    const w = await openDialog();
    await toOptions(w, "invoicer");
    await w.get('[data-kit="vue"]').trigger("click");
    const radio = (label: string, text: string) =>
      w
        .get(`[aria-label="${label}"]`)
        .findAll('[role="radio"]')
        .find((r) => r.text() === text);
    await radio("Authentication provider", "WorkOS")?.trigger("click");
    await radio("Testing framework", "PHPUnit")?.trigger("click");
    await radio("Frontend dependencies", "pnpm")?.trigger("click");
    await w.get('select[aria-label="Database"]').setValue("mysql");
    await w.get('select[aria-label="Node version"]').setValue("20.18.0");
    await w.get('[aria-label="Team support"]').trigger("click");
    await w.get('[aria-label="Initialise git"]').trigger("click");
    await w.get('[aria-label="Laravel Boost"]').trigger("click");
    expect(w.get('[data-testid="ns-command"]').text()).toContain(
      "laravel new invoicer --vue --workos --teams --phpunit --database=mysql --pnpm --boost",
    );
    expect(w.get('[data-testid="ns-command"]').text()).toContain(
      "Installs pnpm, builds with Node 20.18.0, serves on PHP 8.4",
    );
    await click(w, "Create site");
    expect(spec()?.framework.options).toEqual({
      starter_kit: "vue",
      auth: "work_os",
      livewire_class_components: false,
      teams: true,
      testing: "php_unit",
      database: "mysql",
      js: "pnpm",
      git: false,
      boost: true,
      node: "20.18.0",
    });
  });

  it("requires a community package before Create", async () => {
    const w = await openDialog();
    await toOptions(w);
    await w.get('[data-kit="community"]').trigger("click");
    expect(byText(w, "Create site")?.attributes("disabled")).toBeDefined();
    await w.get("#ns-pkg").setValue("acme/kit");
    await click(w, "Create site");
    expect(spec()?.framework.options.starter_kit).toEqual({ community: "acme/kit" });
  });

  it("creates CakePHP from the starter picker", async () => {
    const w = await openDialog();
    await setName(w, "shop");
    (w.findComponent(StarterPicker) as VueWrapper).vm.$emit("update:modelValue", "cakephp");
    await flushPromises();
    await click(w, "Next: Options");
    expect(w.get('[data-testid="ns-composer-note"]').text()).toContain("cakephp/app");
    expect(w.text()).toContain("New CakePHP site");
    await click(w, "Create site");
    expect(spec()).toEqual({
      name: "shop",
      parent_dir: "/srv",
      php: "8.4",
      secure: false,
      framework: { framework: "cakephp" },
    });
  });

  it("shows the URL, the summary and the duration when done", async () => {
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    const done = w.get('[data-testid="ns-done"]');
    expect(done.text()).toContain("blog.test is ready");
    expect(done.text()).toContain("http://localhost:8080/~blog.test");
    expect(w.get('[data-testid="ns-summary"]').text()).toMatch(
      /^Laravel · Pest · SQLite · PHP 8\.4 · took \d+s$/,
    );
  });

  it("opens the new site in its resolved editor", async () => {
    getInstalledIdes.mockResolvedValue([{ id: "zed", label: "Zed" }]);
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    await click(w, "Open in editor");
    expect(openInIde).toHaveBeenCalledWith("blog", "zed");
    await click(w, "Finder");
    expect(openPath).toHaveBeenCalledWith("/srv/blog");
    await click(w, "Open site");
    expect(openInBrowser).toHaveBeenCalledWith("http://localhost:8080/~blog.test");
  });

  it("offers WP Admin on Done for WordPress", async () => {
    const w = await openDialog(["8.4"], "8.4", { initialStarter: "wordpress" });
    await toOptions(w);
    await w.get("#ns-wp-title").setValue("My Blog");
    await click(w, "Create site");
    const footer = w
      .findAll("button")
      .map((b) => b.text().trim())
      .filter((t) => ["Open in editor", "Finder", "WP Admin", "Open site"].includes(t));
    expect(footer).toEqual(["Open in editor", "Finder", "WP Admin", "Open site"]);
  });

  it("returns to Options with the form kept after a failed create", async () => {
    jobStatus.mockResolvedValue(status("failed", "composer exploded"));
    const w = await openDialog();
    await toOptions(w);
    await w.get('[data-kit="react"]').trigger("click");
    await click(w, "Create site");
    expect(w.text()).toContain("composer exploded");
    expect(w.find(".animate-spin").exists()).toBe(false);
    await click(w, "Back");
    expect(w.get('[data-kit="react"]').attributes("aria-pressed")).toBe("true");
    expect(byText(w, "Create site")).toBeTruthy();
  });

  it("resets the form when reopened after a finished create", async () => {
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    expect(w.text()).toContain("blog.test is ready");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect((w.get("#ns-name").element as HTMLInputElement).value).toBe("");
  });
});

describe("NewSiteDialog job lifetime", () => {
  it("keeps polling while closed, announces the result and resumes on reopen", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValue(status("succeeded"));
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    expect(w.find("#ns-name").exists()).toBe(false);
    expect(w.text()).toContain("Creating blog.test");
    await w.setProps({ open: false });
    await vi.waitFor(() => expect(w.emitted("created")).toBeTruthy(), { timeout: 3000 });
    expect(toastSuccess).toHaveBeenCalledWith("blog.test is ready");
    w.unmount();
  });

  it("resumes the progress view when reopened before the daemon returns a job id", async () => {
    let resolveCreate: (value: string) => void = () => {};
    createSite.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      }),
    );
    const w = await openDialog();
    await toOptions(w);
    await byText(w, "Create site")?.trigger("click");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect(w.find("#ns-name").exists()).toBe(false);
    resolveCreate("job-1");
    await flushPromises();
    expect(w.emitted("created")).toBeTruthy();
    w.unmount();
  });

  it("announces a status failure that happens while closed", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockRejectedValue(new Error("daemon restarted"));
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    await w.setProps({ open: false });
    await vi.waitFor(
      () => expect(toastError).toHaveBeenCalledWith("Couldn't create blog.test", "daemon restarted"),
      { timeout: 2000 },
    );
    w.unmount();
  });

  it("keeps the job when unmounted, shows it in a remounted dialog and toasts the finish", async () => {
    jobStatus.mockResolvedValue(status("running"));
    const w = await openDialog(["8.3", "8.4"], "8.4", { initialStarter: "wordpress" });
    await toOptions(w, "shop");
    await w.get("#ns-wp-title").setValue("Shop");
    await click(w, "Create site");
    w.unmount();
    const again = await openDialog();
    expect(again.find("#ns-name").exists()).toBe(false);
    expect(again.text()).toContain("Creating shop.test");
    expect(again.text()).toContain("Downloading WordPress");
    await click(again, "Cancel");
    expect(jobCancel).toHaveBeenCalledWith("job-1");
    await again.setProps({ open: false });
    again.unmount();
    jobStatus.mockResolvedValue(status("succeeded"));
    await vi.waitFor(() => expect(toastSuccess).toHaveBeenCalledWith("shop.test is ready"), {
      timeout: 2000,
    });
  });

  it("never starts a second create while one runs", async () => {
    jobStatus.mockResolvedValue(status("running"));
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    w.unmount();
    const again = await openDialog();
    expect(byText(again, "Create site")).toBeUndefined();
    expect(byText(again, "Next: Options")).toBeUndefined();
    expect(createSite).toHaveBeenCalledTimes(1);
    again.unmount();
  });

  it("keeps a running tools install when reopened instead of starting over", async () => {
    listTools.mockResolvedValue([tool("laravel")]);
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockReturnValue(new Promise(() => {}));
    const w = await openDialog();
    await toOptions(w);
    await byText(w, "Install missing tools")?.trigger("click");
    await flushPromises();
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect(w.get('[data-tool="composer"]').text()).toContain("Installing");
    expect(byText(w, "Installing…")?.attributes("disabled")).toBeDefined();
    expect(installToolStreamed).toHaveBeenCalledTimes(1);
    w.unmount();
  });

  it("re-enables Cancel when a cancelled create is retried", async () => {
    jobStatus
      .mockResolvedValueOnce(status("running"))
      .mockResolvedValueOnce(status("cancelled"))
      .mockResolvedValue(status("running"));
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    await byText(w, "Cancel")?.trigger("click");
    expect(byText(w, "Cancelling")).toBeTruthy();
    await vi.waitFor(() => expect(byText(w, "Back")).toBeTruthy(), { timeout: 2000 });
    expect(w.text()).toContain("Cancelled.");
    await click(w, "Back");
    await click(w, "Create site");
    const cancel = byText(w, "Cancel");
    expect(cancel?.text()).toBe("Cancel");
    expect(cancel?.attributes("disabled")).toBeUndefined();
    w.unmount();
  });

  it("shows the phases with the active one's latest log line", async () => {
    jobStatus.mockResolvedValue({
      log: ["bun install", "resolving 214 packages"],
      next_cursor: 2,
      phase: "Scaffolding",
      state: "running",
      error: null,
    });
    const w = await openDialog();
    await toOptions(w);
    await click(w, "Create site");
    expect(w.get('[data-phase="Preflight"]').find('[aria-label="Done"]').exists()).toBe(true);
    expect(w.get('[data-phase="Scaffolding"]').text()).toContain("resolving 214 packages");
    expect(w.get('[data-phase="Registering"]').text()).toBe("Registering with Yerd");
    expect(w.get('[data-phase="Done"]').text()).toBe("Ready at http://localhost:8080/~blog.test");
    expect(w.get('[data-testid="ns-elapsed"]').text()).toMatch(/^0:0\d$/);
    await click(w, "Hide log");
    expect(w.find("pre").exists()).toBe(false);
    w.unmount();
  });
});
