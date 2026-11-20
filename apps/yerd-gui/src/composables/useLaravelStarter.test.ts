import { flushPromises } from "@vue/test-utils";
import { effectScope, nextTick, reactive, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTools = vi.hoisted(() => vi.fn());
const listNode = vi.hoisted(() => vi.fn());
const availablePhp = vi.hoisted(() => vi.fn());
const installPhpWithProgress = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp,
  installPhpWithProgress,
  installToolStreamed,
  listTools,
  listNode,
  pollJobToEnd,
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import { useLaravelStarter } from "./useLaravelStarter";
import { resetResourceCache } from "./useResource";
import { useToolPrereqs } from "./useToolPrereqs";
import type { NodeRelease, ToolStatus } from "@/ipc/types";

function tool(id: string, overrides: Partial<ToolStatus> = {}): ToolStatus {
  return { id, display_name: id, installed: true, version: "1.0", binaries: [id], ...overrides };
}

function release(version: string, isDefault = false): NodeRelease {
  return { version, lts: true, default: isDefault, uninstall_blocked: false };
}

const scopes: ReturnType<typeof effectScope>[] = [];

async function setup(phpVersions = ["8.3", "8.4"], defaultPhp = "8.4") {
  const basics = reactive({ name: "blog", location: "/srv", php: "", secure: false });
  const versions = ref(phpVersions);
  const scope = effectScope();
  scopes.push(scope);
  const result = scope.run(() => {
    const prereqs = reactive(useToolPrereqs(() => true));
    const starter = reactive(
      useLaravelStarter(basics, {
        phpVersions: () => versions.value,
        defaultPhp: () => defaultPhp,
        prereqs,
        report: () => null,
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

function options(s: Awaited<ReturnType<typeof setup>>["starter"]) {
  const spec = s.buildSpec();
  if (spec.framework.framework !== "laravel") throw new Error("not a Laravel spec");
  return spec.framework.options;
}

beforeEach(() => {
  vi.clearAllMocks();
  resetResourceCache();
  listTools.mockResolvedValue([tool("composer"), tool("laravel"), tool("node")]);
  listNode.mockResolvedValue([]);
});

afterEach(() => {
  scopes.splice(0).forEach((s) => s.stop());
});

describe("useLaravelStarter PHP", () => {
  it("honours a supported default PHP that isn't the newest", async () => {
    const { starter } = await setup(["8.3", "8.4"], "8.3");
    expect(starter.preferredPhp()).toBe("8.3");
  });

  it("defaults to the newest supported PHP when the default is outside 8.3-8.5", async () => {
    const { starter } = await setup(["8.2", "8.3", "8.4"], "8.2");
    expect(starter.preferredPhp()).toBe("8.4");
    expect(starter.supportsPhp("8.2")).toBe(false);
    expect(starter.supportsPhp("8.5")).toBe(true);
    expect(starter.supportsPhp("8.6")).toBe(false);
  });
});

describe("useLaravelStarter spec", () => {
  it("builds the default spec", async () => {
    const { starter } = await setup();
    expect(starter.buildSpec()).toEqual({
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
  });

  it("sends the chosen JS runtime for any kit, including Livewire", async () => {
    const { starter } = await setup();
    starter.kitChoice = "vue";
    starter.form.js = "bun";
    expect(options(starter)).toMatchObject({ starter_kit: "vue", js: "bun" });
    starter.kitChoice = "livewire";
    starter.form.js = "pnpm";
    expect(options(starter)).toMatchObject({ starter_kit: "livewire", js: "pnpm" });
  });

  it("requires a community package", async () => {
    const { starter } = await setup();
    starter.kitChoice = "community";
    expect(starter.optionsValid).toBe(false);
    starter.form.communityPackage = " acme/kit ";
    expect(starter.optionsValid).toBe(true);
    expect(options(starter).starter_kit).toEqual({ community: "acme/kit" });
  });

  it("only sends auth, teams and class components where the kit supports them", async () => {
    const { starter } = await setup();
    starter.form.auth = "work_os";
    starter.form.teams = true;
    starter.form.livewireClassComponents = true;
    expect(options(starter)).toMatchObject({
      auth: "laravel",
      teams: false,
      livewire_class_components: false,
    });
    starter.kitChoice = "livewire";
    expect(options(starter)).toMatchObject({
      auth: "work_os",
      teams: true,
      livewire_class_components: true,
    });
  });

  it("drops teams and class components with no authentication", async () => {
    const { starter } = await setup();
    starter.kitChoice = "livewire";
    starter.form.auth = "none";
    starter.form.teams = true;
    starter.form.livewireClassComponents = true;
    expect(starter.noAuth).toBe(true);
    expect(options(starter)).toMatchObject({
      auth: "none",
      teams: false,
      livewire_class_components: false,
    });
    expect(starter.command).toContain("--livewire --no-authentication");
  });

  it.each([
    ["none", false],
    ["react", true],
    ["vue", true],
    ["livewire", true],
    ["svelte", true],
    ["community", false],
  ] as const)("offers no authentication for the %s kit: %s", async (kit, offered) => {
    const { starter } = await setup();
    starter.kitChoice = kit;
    expect(starter.offersNoAuth).toBe(offered);
  });

  it("resets no authentication to built-in on switching to a community kit", async () => {
    const { starter } = await setup();
    starter.kitChoice = "vue";
    starter.form.auth = "none";
    await nextTick();
    starter.kitChoice = "community";
    expect(starter.noAuth).toBe(false);
    expect(options(starter).auth).toBe("laravel");
    await nextTick();
    expect(starter.form.auth).toBe("laravel");
  });
});

describe("useLaravelStarter Node", () => {
  it("hides the Node picker with fewer than two Yerd releases", async () => {
    for (const installed of [[], [release("22.12.0", true)]]) {
      listNode.mockResolvedValue(installed);
      resetResourceCache();
      const { starter } = await setup();
      starter.form.node = "22.12.0";
      expect(starter.showNodePicker).toBe(false);
      expect(options(starter).node).toBeUndefined();
    }
  });

  it("sends the picked Node release and omits it for Default", async () => {
    listNode.mockResolvedValue([release("24.2.0", true), release("22.12.0")]);
    const { starter } = await setup();
    expect(starter.nodeOptions.map((o) => o.label)).toEqual([
      "Default (Node 24.2.0)",
      "Node 24.2.0",
      "Node 22.12.0",
    ]);
    expect(options(starter).node).toBeUndefined();
    starter.form.node = "22.12.0";
    expect(options(starter).node).toBe("22.12.0");
  });

  it("hides the Node picker when frontend dependencies are skipped", async () => {
    listNode.mockResolvedValue([release("24.2.0", true), release("22.12.0")]);
    const { starter } = await setup();
    starter.form.js = "skip";
    expect(starter.showNodePicker).toBe(false);
  });
});

describe("useLaravelStarter auto-install note", () => {
  it.each([
    { label: "Node missing for npm", tools: [tool("composer"), tool("laravel")], js: "npm", note: "Node" },
    { label: "skipped", tools: [tool("composer"), tool("laravel")], js: "skip", note: null },
    {
      label: "pnpm missing",
      tools: [tool("composer"), tool("laravel"), tool("node")],
      js: "pnpm",
      note: "pnpm",
    },
    {
      label: "Node and pnpm missing",
      tools: [tool("composer"), tool("laravel")],
      js: "pnpm",
      note: "Node and pnpm",
    },
    {
      label: "external Node, pnpm present",
      tools: [
        tool("composer"),
        tool("laravel"),
        tool("node", { installed: false, external: true }),
        tool("pnpm"),
      ],
      js: "pnpm",
      note: null,
    },
  ] as const)("names what the job installs ($label)", async ({ tools, js, note }) => {
    listTools.mockResolvedValue(tools);
    const { starter } = await setup();
    starter.form.js = js;
    expect(starter.autoInstallLabel).toBe(note);
  });
});

describe("useLaravelStarter command strip", () => {
  it("previews the laravel new command and what Yerd does around it", async () => {
    listTools.mockResolvedValue([tool("composer"), tool("laravel")]);
    listNode.mockResolvedValue([release("22.11.0", true), release("20.18.0")]);
    const { starter, basics } = await setup(["8.4", "8.5"], "8.5");
    basics.name = "Invoicer";
    starter.kitChoice = "vue";
    starter.form.database = "mysql";
    starter.form.js = "pnpm";
    starter.form.git = false;
    expect(starter.command).toBe("laravel new invoicer --vue --pest --database=mysql --pnpm");
    expect(starter.note).toBe(
      "Installs Node and pnpm, builds with Node 22.11.0, serves on PHP 8.5",
    );
    starter.form.js = "skip";
    expect(starter.note).toBe("Serves on PHP 8.5");
  });

  it("summarises the Done screen", async () => {
    const { starter, basics } = await setup();
    basics.php = "8.5";
    expect(starter.doneSummary).toBe("Laravel · Pest · SQLite · PHP 8.5");
    starter.kitChoice = "vue";
    starter.form.database = "mysql";
    expect(starter.doneSummary).toBe("Laravel · Vue · Pest · MySQL · PHP 8.5");
  });

  it("restores every option on reset", async () => {
    const { starter } = await setup();
    starter.kitChoice = "react";
    starter.form.testing = "php_unit";
    starter.form.js = "bun";
    starter.form.git = false;
    starter.reset();
    expect(starter.kitChoice).toBe("none");
    expect(options(starter)).toMatchObject({ testing: "pest", js: "npm", git: true });
  });
});

describe("useLaravelStarter toolchain", () => {
  it("is ready with PHP, Composer and the installer", async () => {
    const { starter } = await setup();
    expect(starter.ready).toBe(true);
  });

  it("blocks the installer row until Yerd's own Composer is present", async () => {
    listTools.mockResolvedValue([tool("composer", { installed: false, external: true })]);
    const { starter } = await setup();
    expect(starter.ready).toBe(false);
    const row = starter.prereqRows.find((r) => r.id === "laravel");
    expect(row?.blockedReason).toContain("Composer is required");
    await starter.installAll();
    expect(toastError).toHaveBeenCalledWith("Can't install the Laravel installer", expect.any(String));
    expect(installToolStreamed).not.toHaveBeenCalled();
  });

  it("explains an installed PHP outside the window", async () => {
    const { starter } = await setup(["8.2"], "8.2");
    expect(starter.prereqRows[0]?.sub).toBe("Installed, but not 8.3-8.5");
    expect(starter.ready).toBe(false);
  });

  it("installs missing tools in dependency order", async () => {
    listTools.mockResolvedValueOnce([]).mockResolvedValue([tool("composer")]);
    availablePhp.mockResolvedValue({ available: ["8.2", "8.4", "8.6"] });
    installToolStreamed.mockImplementation(async (id: string) => `install-${id}`);
    pollJobToEnd.mockResolvedValue({ state: "succeeded", error: null });
    const { starter } = await setup([], "");
    await starter.installAll();
    expect(installPhpWithProgress).toHaveBeenCalledWith("8.4", expect.any(Function));
    expect(installToolStreamed.mock.calls.map((c) => c[0])).toEqual(["composer", "laravel"]);
    expect(toastSuccess).toHaveBeenCalledWith("Toolchain ready");
  });
});
