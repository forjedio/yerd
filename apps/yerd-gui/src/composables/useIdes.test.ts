import { afterEach, describe, expect, it, vi } from "vitest";

const getInstalledIdes = vi.fn();
vi.mock("@/ipc/client", () => ({
  getInstalledIdes: (...args: unknown[]) => getInstalledIdes(...args),
}));

import { rescanIdes, resetIdes, useIdes } from "./useIdes";
import type { IdeOption } from "@/ipc/types";

function ide(id: string): IdeOption {
  return { id, label: id } as IdeOption;
}

afterEach(() => {
  resetIdes();
  getInstalledIdes.mockReset();
});

describe("rescanIdes", () => {
  it("caches the detected editors and reports how many were found", async () => {
    getInstalledIdes.mockResolvedValue([ide("vscode"), ide("phpstorm")]);

    await expect(rescanIdes()).resolves.toBe(2);
    expect(useIdes().installedIdes.value.map((i) => i.id)).toEqual(["vscode", "phpstorm"]);
  });

  it("reports zero when the host has no editors", async () => {
    getInstalledIdes.mockResolvedValue([]);

    await expect(rescanIdes()).resolves.toBe(0);
  });

  it("reports the newer result when a later scan supersedes it", async () => {
    let releaseFirst!: (v: IdeOption[]) => void;
    getInstalledIdes.mockReturnValueOnce(
      new Promise<IdeOption[]>((resolve) => {
        releaseFirst = resolve;
      }),
    );
    const first = rescanIdes();

    getInstalledIdes.mockResolvedValue([ide("zed")]);
    await expect(rescanIdes()).resolves.toBe(1);

    releaseFirst([ide("vscode"), ide("phpstorm")]);
    await expect(first).resolves.toBe(1);
    expect(useIdes().installedIdes.value.map((i) => i.id)).toEqual(["zed"]);
  });
});
