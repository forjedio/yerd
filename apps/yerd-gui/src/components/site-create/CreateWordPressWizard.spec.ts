import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import CreateWordPressWizard from "./CreateWordPressWizard.vue";
import type { ServiceStatus, ToolStatus } from "@/ipc/types";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());
const listTools = vi.hoisted(() => vi.fn());
const listServices = vi.hoisted(() => vi.fn());
const availableWordPressVersions = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp: vi.fn(),
  availableWordPressVersions,
  createSite,
  installPhpWithProgress: vi.fn(),
  installToolStreamed,
  jobCancel: vi.fn().mockResolvedValue(undefined),
  jobStatus,
  listServices,
  listTools,
  mintWordPressLoginToken: vi.fn(),
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

function service(id: string, overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: id,
    display_name: id,
    installed_versions: ["1"],
    selected_version: "1",
    state: "stopped",
    pid: null,
    listen: null,
    port: 3306,
    enabled: true,
    supports_databases: true,
    ...overrides,
  };
}

function status(state: string, error: string | null = null) {
  return { log: [], next_cursor: 0, phase: "Installing", state, error };
}

async function openWizard(phpVersions = ["8.3", "8.4"], defaultPhp = "") {
  const w = mount(CreateWordPressWizard, {
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

type Wizard = Awaited<ReturnType<typeof openWizard>>;

function byText(w: Wizard, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

async function next(w: Wizard) {
  await byText(w, "Next")!.trigger("click");
}

async function toReview(w: Wizard, name = "blog") {
  await w.find("#wp-cs-name").setValue(name);
  await next(w);
  await w.find("#wp-cs-title").setValue("My Blog");
  await next(w);
  await next(w);
}

async function create(w: Wizard) {
  await byText(w, "Create site")!.trigger("click");
  await flushPromises();
}

describe("CreateWordPressWizard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listTools.mockResolvedValue([tool("wp-cli")]);
    listServices.mockResolvedValue([]);
    availableWordPressVersions.mockResolvedValue([]);
    createSite.mockResolvedValue("job-1");
    jobStatus.mockResolvedValue(status("succeeded"));
  });

  it("is titled for WordPress", async () => {
    const w = await openWizard();
    expect(w.text()).toContain("Create a new WordPress site");
  });

  it("submits the default spec with a derived database name on the first PHP", async () => {
    const w = await openWizard();
    await toReview(w, "my-blog");
    await create(w);

    expect(createSite).toHaveBeenCalledWith({
      name: "my-blog",
      parent_dir: "/srv",
      php: "8.3",
      secure: false,
      framework: {
        framework: "wordpress",
        options: {
          core_version: null,
          locale: "en_US",
          admin_user: "admin",
          admin_email: "test@example.com",
          admin_password: expect.stringMatching(/^.{20}$/),
          site_title: "My Blog",
          table_prefix: "wp_",
          database: { engine: "mysql", name: "my_blog" },
        },
      },
    });
    expect(w.emitted("created")).toBeTruthy();
  });

  it("prefers the configured default PHP", async () => {
    const w = await openWizard(["8.3", "8.4"], "8.4");
    expect((w.find("#wp-cs-php").element as HTMLSelectElement).value).toBe("8.4");
  });

  it("only fills an empty PHP selection when the installed versions change", async () => {
    const empty = await openWizard([], "");
    await empty.setProps({ phpVersions: ["8.2", "8.4"] });
    expect((empty.find("#wp-cs-php").element as HTMLSelectElement).value).toBe("8.2");

    const w = await openWizard(["8.3", "8.4"], "");
    await w.setProps({ phpVersions: ["8.4"] });
    await toReview(w);
    expect(w.text()).toContain("8.3");
  });

  it("labels legacy PHP and explains it", async () => {
    const w = await openWizard(["7.4", "8.4"], "8.4");
    expect(w.find("#wp-cs-php").text()).toContain("PHP 7.4 (legacy)");
    expect(w.text()).not.toContain("Out of support");
    await w.find("#wp-cs-php").setValue("7.4");
    expect(w.text()).toContain("Out of support");
  });

  it("gates the WordPress and Database steps", async () => {
    const w = await openWizard();
    await w.find("#wp-cs-name").setValue("blog");
    await next(w);
    expect(byText(w, "Next")!.attributes("disabled")).toBeDefined();
    await w.find("#wp-cs-title").setValue("My Blog");
    await w.find("#wp-cs-admin-email").setValue("nope");
    expect(byText(w, "Next")!.attributes("disabled")).toBeDefined();
    await w.find("#wp-cs-admin-email").setValue("a@b.co");
    await next(w);
    await w.find("#wp-cs-dbname").setValue("1bad");
    expect(byText(w, "Next")!.attributes("disabled")).toBeDefined();
  });

  it("keeps a database name the user edited", async () => {
    const w = await openWizard();
    await w.find("#wp-cs-name").setValue("blog");
    await next(w);
    await w.find("#wp-cs-title").setValue("My Blog");
    await next(w);
    await w.find("#wp-cs-dbname").setValue("custom_db");
    await byText(w, "Back")!.trigger("click");
    await byText(w, "Back")!.trigger("click");
    await w.find("#wp-cs-name").setValue("renamed");
    await next(w);
    await next(w);
    expect((w.find("#wp-cs-dbname").element as HTMLInputElement).value).toBe("custom_db");
  });

  it("preselects a running database engine", async () => {
    listServices.mockResolvedValue([
      service("mysql"),
      service("mariadb", { state: "running" }),
    ]);
    const w = await openWizard();
    await toReview(w);
    await create(w);
    expect(createSite.mock.calls[0][0].framework.options.database.engine).toBe("mariadb");
  });

  it("clears the core version when the PHP change makes it incompatible", async () => {
    availableWordPressVersions.mockResolvedValue([
      { branch: "6.7", latest: "6.7.2", min_php: "7.2", max_php: "8.3" },
    ]);
    const w = await openWizard(["8.3", "8.4"], "8.3");
    await w.find("#wp-cs-name").setValue("blog");
    await next(w);
    await w.find("#wp-cs-version").setValue("6.7.2");
    await byText(w, "Back")!.trigger("click");
    await w.find("#wp-cs-php").setValue("8.4");
    await next(w);
    await w.find("#wp-cs-title").setValue("My Blog");
    await next(w);
    await next(w);
    expect(w.text()).toContain("Latest · en_US");
    await create(w);
    expect(createSite.mock.calls[0][0].framework.options.core_version).toBeNull();
  });

  it("keeps a compatible core version", async () => {
    availableWordPressVersions.mockResolvedValue([
      { branch: "6.7", latest: "6.7.2", min_php: "7.2", max_php: "8.4" },
    ]);
    const w = await openWizard(["8.3", "8.4"], "8.3");
    await w.find("#wp-cs-name").setValue("blog");
    await next(w);
    await w.find("#wp-cs-version").setValue("6.7.2");
    await byText(w, "Back")!.trigger("click");
    await w.find("#wp-cs-php").setValue("8.4");
    await next(w);
    await w.find("#wp-cs-title").setValue("My Blog");
    await next(w);
    await next(w);
    await create(w);
    expect(createSite.mock.calls[0][0].framework.options.core_version).toBe("6.7.2");
  });

  it("lists Composer only while WP-CLI still has to be built", async () => {
    listTools.mockResolvedValue([tool("composer")]);
    const w = await openWizard();
    expect(w.text()).toContain("Builds WP-CLI");

    listTools.mockResolvedValue([tool("wp-cli")]);
    const noPhp = await openWizard([], "");
    expect(noPhp.text()).toContain("A few tools are needed first");
    expect(noPhp.text()).not.toContain("Builds WP-CLI");
  });

  it("blocks the WP-CLI row until Yerd's own Composer is present", async () => {
    listTools.mockResolvedValue([tool("composer", { installed: false, external: true })]);
    const w = await openWizard();
    const installs = w.findAll("button").filter((b) => b.text() === "Install");
    const install = installs[installs.length - 1];
    expect(install.attributes("disabled")).toBeDefined();
    expect(install.attributes("title")).toContain("Composer is required to build WP-CLI");
  });

  it("installs Composer before WP-CLI", async () => {
    listTools.mockResolvedValueOnce([]).mockResolvedValue([tool("composer")]);
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockResolvedValue({ state: "succeeded", error: null });
    const w = await openWizard();

    await byText(w, "Install missing tools")!.trigger("click");
    await flushPromises();
    expect(installToolStreamed.mock.calls.map((c) => c[0])).toEqual(["composer", "wp-cli"]);
    expect(toastSuccess).toHaveBeenCalledWith("Toolchain ready");
  });

  it("offers WP Admin between Open folder and Open in browser", async () => {
    const w = await openWizard();
    await toReview(w);
    await create(w);
    const footer = w
      .findAll("button")
      .map((b) => b.text().trim())
      .filter((t) => ["Open folder", "WP Admin", "Open in browser", "Done"].includes(t));
    expect(footer).toEqual(["Open folder", "WP Admin", "Open in browser", "Done"]);
  });

  it("returns to Review with the form kept after a failed create", async () => {
    jobStatus.mockResolvedValue(status("failed", "download failed"));
    const w = await openWizard();
    await toReview(w);
    await create(w);

    expect(w.text()).toContain("download failed");
    await byText(w, "Back")!.trigger("click");
    expect(w.text()).toContain("blog.test");
    expect(byText(w, "Create site")).toBeTruthy();
  });

  it("resets the form when reopened after a finished create", async () => {
    const w = await openWizard();
    await toReview(w);
    await create(w);
    expect(w.text()).toContain("blog.test is ready");

    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await flushPromises();
    expect((w.find("#wp-cs-name").element as HTMLInputElement).value).toBe("");
  });
});
