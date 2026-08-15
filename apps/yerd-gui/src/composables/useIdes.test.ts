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

  it("waits for the newer scan when the superseded one resolves first", async () => {
    getInstalledIdes.mockResolvedValueOnce([ide("vscode"), ide("phpstorm")]);
    let releaseSecond!: (v: IdeOption[]) => void;
    getInstalledIdes.mockReturnValueOnce(
      new Promise<IdeOption[]>((resolve) => {
        releaseSecond = resolve;
      }),
    );

    const first = rescanIdes();
    const second = rescanIdes();

    releaseSecond([ide("zed")]);
    await expect(first).resolves.toBe(1);
    await expect(second).resolves.toBe(1);
    expect(useIdes().installedIdes.value.map((i) => i.id)).toEqual(["zed"]);
  });

  it("falls back to the cache when a reset supersedes an in-flight scan", async () => {
    let release!: (v: IdeOption[]) => void;
    getInstalledIdes.mockReturnValueOnce(
      new Promise<IdeOption[]>((resolve) => {
        release = resolve;
      }),
    );
    const scan = rescanIdes();

    resetIdes();
    release([ide("vscode")]);

    await expect(scan).resolves.toBe(0);
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
