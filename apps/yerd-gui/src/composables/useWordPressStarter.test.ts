import { flushPromises } from "@vue/test-utils";
import { effectScope, nextTick, reactive, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTools = vi.hoisted(() => vi.fn());
const listServices = vi.hoisted(() => vi.fn());
const availableWordPressVersions = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const mintWordPressLoginToken = vi.hoisted(() => vi.fn());
const openInBrowser = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp: vi.fn(),
  availableWordPressVersions,
  installPhpWithProgress: vi.fn(),
  installToolStreamed,
  listServices,
  listTools,
  mintWordPressLoginToken,
  openInBrowser,
  pollJobToEnd,
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import { deriveDbName, useWordPressStarter } from "./useWordPressStarter";
import { useToolPrereqs } from "./useToolPrereqs";
import type { ServiceStatus, StatusReport, ToolStatus } from "@/ipc/types";

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

const scopes: ReturnType<typeof effectScope>[] = [];

async function setup(
  phpVersions = ["8.3", "8.4"],
  defaultPhp = "",
  name = "",
  report: StatusReport | null = null,
) {
  const basics = reactive({ name, location: "/srv", php: "", secure: false });
  const versions = ref(phpVersions);
  const scope = effectScope();
  scopes.push(scope);
  const result = scope.run(() => {
    const prereqs = reactive(useToolPrereqs(() => true));
    const starter = reactive(
      useWordPressStarter(basics, {
        phpVersions: () => versions.value,
        defaultPhp: () => defaultPhp,
        prereqs,
        report: () => report,
      }),
    );
    return { prereqs, starter };
  });
  if (!result) throw new Error("setup failed");
  await result.prereqs.refresh();
  result.starter.reset();
  basics.php = result.starter.preferredPhp();
  await flushPromises();
  return { basics, versions, ...result };
}

function wpOptions(s: Awaited<ReturnType<typeof setup>>["starter"]) {
  const spec = s.buildSpec();
  if (spec.framework.framework !== "wordpress") throw new Error("not a WordPress spec");
  return spec.framework.options;
}

beforeEach(() => {
  vi.clearAllMocks();
  listTools.mockResolvedValue([tool("wp-cli")]);
  listServices.mockResolvedValue([]);
  availableWordPressVersions.mockResolvedValue([]);
});

afterEach(() => {
  scopes.splice(0).forEach((s) => s.stop());
});

describe("useWordPressStarter", () => {
  it("builds the default spec with a derived database name on the first PHP", async () => {
    const { starter, basics } = await setup();
    basics.name = "my-blog";
    await nextTick();
    starter.form.siteTitle = "My Blog";
    expect(starter.buildSpec()).toEqual({
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
  });

  it("prefers the configured default PHP and supports every version", async () => {
    const { starter } = await setup(["8.3", "8.4"], "8.4");
    expect(starter.preferredPhp()).toBe("8.4");
    expect(starter.supportsPhp("7.4")).toBe(true);
  });

  it("never prefers a default PHP that isn't installed", async () => {
    const { starter } = await setup(["7.4", "8.3"], "8.4");
    expect(starter.preferredPhp()).toBe("7.4");
  });

  it("labels and explains legacy PHP", async () => {
    const { starter } = await setup(["7.4", "8.4"], "8.4");
    expect(starter.phpLabel("7.4")).toBe("7.4 (legacy)");
    expect(starter.phpLabel("8.4")).toBe("8.4");
    expect(starter.phpHint("7.4")).toContain("Out of support");
    expect(starter.phpHint("8.4")).not.toContain("Out of support");
  });

  it("gates the options on a title, a valid email, a password and a valid database name", async () => {
    const { starter, basics } = await setup();
    basics.name = "blog";
    await nextTick();
    expect(starter.optionsValid).toBe(false);
    starter.form.siteTitle = "My Blog";
    expect(starter.optionsValid).toBe(true);
    starter.form.adminEmail = "nope";
    expect(starter.optionsValid).toBe(false);
    starter.form.adminEmail = "a@b.co";
    starter.form.adminPassword = "short";
    expect(starter.optionsValid).toBe(false);
    starter.regeneratePassword();
    starter.form.dbName = "1bad";
    expect(starter.optionsValid).toBe(false);
  });

  it("keeps a database name the user edited", async () => {
    const { starter, basics } = await setup();
    basics.name = "blog";
    await nextTick();
    starter.form.dbName = "custom_db";
    starter.touchDbName();
    basics.name = "renamed";
    await nextTick();
    expect(starter.form.dbName).toBe("custom_db");
  });

  it("derives the database name from a name typed before reset", async () => {
    const { starter, basics } = await setup([], "", "my-shop");
    expect(starter.form.dbName).toBe("my_shop");
    starter.form.dbName = "custom";
    starter.touchDbName();
    starter.reset();
    expect(starter.form.dbName).toBe("my_shop");
    basics.name = "other";
    await nextTick();
    expect(starter.form.dbName).toBe("other");
  });

  it.each([
    ["blog", "blog"],
    ["my-blog", "my_blog"],
    ["1blog", "wp_1blog"],
    ["a".repeat(70), "a".repeat(63)],
  ])("derives %s as %s", (name, expected) => {
    expect(deriveDbName(name)).toBe(expected);
  });

  it("preselects a running database engine", async () => {
    listServices.mockResolvedValue([service("mysql"), service("mariadb", { state: "running" })]);
    const { starter } = await setup();
    expect(wpOptions(starter).database.engine).toBe("mariadb");
    expect(starter.engineStateText).toBe("mariadb is running and ready.");
  });

  it("keeps an engine the user picked", async () => {
    listServices.mockResolvedValue([service("mariadb", { state: "running" })]);
    const { starter } = await setup();
    starter.selectDbEngine("mysql");
    expect(wpOptions(starter).database.engine).toBe("mysql");
  });

  it("clears the core version when the PHP change makes it incompatible", async () => {
    availableWordPressVersions.mockResolvedValue([
      { branch: "6.7", latest: "6.7.2", min_php: "7.2", max_php: "8.3" },
    ]);
    const { starter, basics } = await setup(["8.3", "8.4"], "8.3");
    starter.form.coreVersion = "6.7.2";
    basics.php = "8.4";
    await nextTick();
    expect(wpOptions(starter).core_version).toBeNull();
    expect(starter.versionOptions.map((o) => o.label)).toEqual(["Latest"]);
  });

  it("keeps a compatible core version", async () => {
    availableWordPressVersions.mockResolvedValue([
      { branch: "6.7", latest: "6.7.2", min_php: "7.2", max_php: "8.4" },
    ]);
    const { starter, basics } = await setup(["8.3", "8.4"], "8.3");
    starter.form.coreVersion = "6.7.2";
    basics.php = "8.4";
    await nextTick();
    expect(wpOptions(starter).core_version).toBe("6.7.2");
  });

  it("summarises the Done screen", async () => {
    const { starter, basics } = await setup();
    basics.php = "8.4";
    expect(starter.doneSummary).toBe("WordPress latest · en_US · MySQL · PHP 8.4");
  });

  it("opens a signed-in WP Admin when bound, the plain link otherwise", async () => {
    const bound = {
      resolver_installed: true,
      tld: "test",
      http: { requested: 80, bound: 80, fell_back: false },
      https: { requested: 443, bound: 443, fell_back: false },
    } as StatusReport;
    mintWordPressLoginToken.mockResolvedValue("tok");
    const { starter } = await setup(["8.4"], "", "blog", bound);
    await starter.openWpAdmin();
    expect(openInBrowser).toHaveBeenCalledWith("http://blog.test/wp-admin/?yerd_login_token=tok");

    const unbound = await setup(["8.4"], "", "blog", null);
    await unbound.starter.openWpAdmin();
    expect(openInBrowser).toHaveBeenLastCalledWith("http://localhost:8080/~blog.test/wp-admin/");
  });
});

describe("useWordPressStarter toolchain", () => {
  it("lists Composer only while WP-CLI still has to be built", async () => {
    listTools.mockResolvedValue([tool("composer")]);
    const missing = await setup();
    expect(missing.starter.prereqRows.map((r) => r.sub)).toContain("Builds WP-CLI");

    listTools.mockResolvedValue([tool("wp-cli")]);
    const noPhp = await setup([], "");
    expect(noPhp.starter.ready).toBe(false);
    expect(noPhp.starter.prereqRows.map((r) => r.sub)).not.toContain("Builds WP-CLI");
  });

  it("blocks the WP-CLI row until Yerd's own Composer is present", async () => {
    listTools.mockResolvedValue([tool("composer", { installed: false, external: true })]);
    const { starter } = await setup();
    const row = starter.prereqRows.find((r) => r.id === "wp-cli");
    expect(row?.blockedReason).toContain("Composer is required to build WP-CLI");
  });

  it("installs Composer before WP-CLI", async () => {
    listTools.mockResolvedValueOnce([]).mockResolvedValue([tool("composer")]);
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockResolvedValue({ state: "succeeded", error: null });
    const { starter } = await setup();
    await starter.installAll();
    expect(installToolStreamed.mock.calls.map((c) => c[0])).toEqual(["composer", "wp-cli"]);
    expect(toastSuccess).toHaveBeenCalledWith("Toolchain ready");
  });
});
