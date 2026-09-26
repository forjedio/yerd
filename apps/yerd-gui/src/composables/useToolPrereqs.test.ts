import { beforeEach, describe, expect, it, vi } from "vitest";

const listTools = vi.hoisted(() => vi.fn());
const availablePhp = vi.hoisted(() => vi.fn());
const installPhpWithProgress = vi.hoisted(() => vi.fn());
const installToolStreamed = vi.hoisted(() => vi.fn());
const pollJobToEnd = vi.hoisted(() => vi.fn());
const refreshDaemon = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  availablePhp,
  installPhpWithProgress,
  installToolStreamed,
  listTools,
  pollJobToEnd,
}));
vi.mock("@/composables/useDaemon", () => ({
  useDaemon: () => ({ refresh: refreshDaemon }),
}));
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import { useToolPrereqs } from "./useToolPrereqs";
import type { ToolStatus } from "@/ipc/types";

function tool(id: string, overrides: Partial<ToolStatus> = {}): ToolStatus {
  return { id, display_name: id, installed: true, version: "1.0", binaries: [id], ...overrides };
}

beforeEach(() => {
  vi.clearAllMocks();
  listTools.mockResolvedValue([]);
  installToolStreamed.mockResolvedValue("job-1");
});

describe("useToolPrereqs", () => {
  it("tells managed tools apart from ones found on PATH", async () => {
    listTools.mockResolvedValue([
      tool("composer", { installed: false, external: true }),
      tool("wp-cli"),
    ]);
    const p = useToolPrereqs(() => true);
    await p.refresh();

    expect(p.available("composer")).toBe(true);
    expect(p.managed("composer")).toBe(false);
    expect(p.managed("wp-cli")).toBe(true);
    expect(p.available("laravel")).toBe(false);
  });

  it("treats a failed tool listing as nothing installed", async () => {
    listTools.mockRejectedValue(new Error("daemon down"));
    const p = useToolPrereqs(() => true);
    await p.refresh();
    expect(p.tools.value).toEqual([]);
    expect(p.loading.value).toBe(false);
  });

  it.each([
    ["succeeded", null, true, false],
    ["failed", "no network", false, true],
    ["running", null, false, false],
  ] as const)(
    "installTool reports a %s install",
    async (state, error, expected, toasted) => {
      pollJobToEnd.mockImplementation(async (_id, onLines: (l: string[]) => void) => {
        onLines(["fetching"]);
        return { state, error };
      });
      const p = useToolPrereqs(() => true);

      await expect(p.installTool("composer")).resolves.toBe(expected);
      expect(p.installLog.value).toEqual(["fetching"]);
      expect(p.installingTool.value).toBeNull();
      expect(listTools).toHaveBeenCalled();
      expect(toastError.mock.calls.length > 0).toBe(toasted);
    },
  );

  it("installs the newest PHP the wizard accepts", async () => {
    availablePhp.mockResolvedValue({ available: ["8.1", "8.3", "8.4", "8.6"] });
    const p = useToolPrereqs(() => true);

    await expect(p.installPhp((v) => v !== "8.6", "none")).resolves.toBe(true);
    expect(installPhpWithProgress).toHaveBeenCalledWith("8.4", expect.any(Function));
    expect(refreshDaemon).toHaveBeenCalled();
    expect(p.installLog.value).toEqual(["Installing PHP 8.4…", "Installed PHP 8.4"]);
  });

  it("explains when no installable PHP is accepted", async () => {
    availablePhp.mockResolvedValue({ available: ["7.4"] });
    const p = useToolPrereqs(() => true);

    await expect(p.installPhp(() => false, "Nothing suitable.")).resolves.toBe(false);
    expect(toastError).toHaveBeenCalledWith("Couldn't install PHP", "Nothing suitable.");
    expect(installPhpWithProgress).not.toHaveBeenCalled();
  });

  it.each([
    [true, 1],
    [false, 0],
  ])("installAll announces the toolchain only when the run succeeds (%s)", async (ok, toasts) => {
    const p = useToolPrereqs(() => true);
    let busyDuringRun = false;

    await p.installAll(async () => {
      busyDuringRun = p.busy.value;
      return ok;
    });

    expect(busyDuringRun).toBe(true);
    expect(p.installingAll.value).toBe(false);
    expect(toastSuccess).toHaveBeenCalledTimes(toasts);
  });
});
