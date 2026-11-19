import { effectScope, reactive } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTools = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp: vi.fn(),
  installPhpWithProgress: vi.fn(),
  installToolStreamed: vi.fn(),
  listTools,
  pollJobToEnd: vi.fn(),
}));

vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn() }),
}));

import { preferredPhp, useComposerStarter } from "./useSiteStarters";
import { useToolPrereqs } from "./useToolPrereqs";
import type { ComposerFramework, ToolStatus } from "@/ipc/types";

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

const scopes: ReturnType<typeof effectScope>[] = [];

async function setup(framework: ComposerFramework, phpVersions: string[], defaultPhp = "") {
  const basics = reactive({ name: "shop", location: "/srv", php: "", secure: false });
  const scope = effectScope();
  scopes.push(scope);
  const result = scope.run(() => {
    const prereqs = reactive(useToolPrereqs(() => true));
    const starter = reactive(
      useComposerStarter(framework, basics, {
        phpVersions: () => phpVersions,
        defaultPhp: () => defaultPhp,
        prereqs,
        report: () => null,
      }),
    );
    return { prereqs, starter };
  });
  if (!result) throw new Error("setup failed");
  await result.prereqs.refresh();
  basics.php = result.starter.preferredPhp();
  return { basics, ...result };
}

beforeEach(() => {
  listTools.mockReset().mockResolvedValue([composer()]);
});

afterEach(() => {
  scopes.splice(0).forEach((s) => s.stop());
});

describe("preferredPhp", () => {
  const any = () => true;
  const from84 = (v: string) => v >= "8.4";
  it.each([
    [["8.3", "8.4"], "8.3", any, "8.3"],
    [["8.4", "8.3"], "", any, "8.4"],
    [["8.3", "8.4"], "8.3", from84, "8.4"],
    [["8.3"], "8.3", from84, ""],
    [[], "", any, ""],
  ] as const)("picks from %o with default %s", (versions, def, supports, expected) => {
    expect(preferredPhp(versions, def, supports)).toBe(expected);
  });
});

describe("useComposerStarter", () => {
  it.each([
    ["codeigniter", "CodeIgniter", "codeigniter4/appstarter"],
    ["cakephp", "CakePHP", "cakephp/app"],
    ["slim", "Slim", "slim/skeleton"],
  ] as const)("names %s after the framework and its package", async (framework, label, pkg) => {
    const { starter } = await setup(framework, ["8.4"]);
    expect(starter.label).toBe(label);
    expect(starter.pkg).toBe(pkg);
    expect(starter.prereqSummary).toBe(`Creating a ${label} site needs PHP and Composer.`);
  });

  it("builds a spec tagged with the framework on the newest supported PHP", async () => {
    const { starter } = await setup("cakephp", ["8.3", "8.4"]);
    expect(starter.buildSpec()).toEqual({
      name: "shop",
      parent_dir: "/srv",
      php: "8.4",
      secure: false,
      framework: { framework: "cakephp" },
    });
    expect(starter.doneSummary).toBe("CakePHP · PHP 8.4");
  });

  it("gates on PHP when every installed version is below the framework minimum", async () => {
    const { starter } = await setup("codeigniter", ["7.4"]);
    expect(starter.ready).toBe(false);
    expect(starter.prereqRows[0]?.sub).toBe("Installed, but not 8.1+");
  });

  it("accepts older PHP for Slim", async () => {
    const { starter } = await setup("slim", ["7.4"]);
    expect(starter.ready).toBe(true);
    expect(starter.supportsPhp("7.4")).toBe(true);
  });

  it("treats an external Composer as satisfying the prerequisite", async () => {
    listTools.mockResolvedValue([composer({ installed: false, external: true })]);
    const { starter } = await setup("slim", ["8.4"]);
    expect(starter.ready).toBe(true);
  });

  it("gates on Composer when it is neither managed nor external", async () => {
    listTools.mockResolvedValue([]);
    const { starter } = await setup("slim", ["8.4"]);
    expect(starter.ready).toBe(false);
    expect(starter.prereqRows.find((r) => r.id === "composer")?.ok).toBe(false);
  });

  it("has no options to validate", async () => {
    const { starter } = await setup("slim", ["8.4"]);
    expect(starter.optionsValid).toBe(true);
  });
});
