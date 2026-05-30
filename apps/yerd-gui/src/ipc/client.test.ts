import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke surface before importing the client.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
  IpcError,
  listPhp,
  listSites,
  status,
  unlink,
  updatePhp,
} from "./client";
import type { Site, StatusReport } from "./types";

beforeEach(() => invokeMock.mockReset());

const sampleSite: Site = {
  name: "blog",
  document_root: "/srv/blog",
  php: "8.5",
  secure: true,
  kind: "linked",
};

describe("client → command mapping", () => {
  it("listSites unwraps the sites array", async () => {
    invokeMock.mockResolvedValue({ type: "sites", sites: [sampleSite] });
    await expect(listSites()).resolves.toEqual([sampleSite]);
    expect(invokeMock).toHaveBeenCalledWith("list_sites", undefined);
  });

  it("status returns the inner report", async () => {
    const report = { daemon_pid: 7, tld: "test" } as unknown as StatusReport;
    invokeMock.mockResolvedValue({ type: "status", report });
    await expect(status()).resolves.toBe(report);
  });

  it("listPhp passes through installed/default/updates", async () => {
    invokeMock.mockResolvedValue({
      type: "php_versions",
      installed: ["8.4", "8.5"],
      default: "8.5",
      updates: [{ version: "8.5", installed: "8.5.6", latest: "8.5.7" }],
    });
    const r = await listPhp();
    expect(r.installed).toEqual(["8.4", "8.5"]);
    expect(r.updates?.[0].latest).toBe("8.5.7");
  });

  it("updatePhp(null) sends a null version (update-all)", async () => {
    invokeMock.mockResolvedValue({ type: "ok" });
    await updatePhp(null);
    expect(invokeMock).toHaveBeenCalledWith("update_php", { version: null });
  });
});

describe("error handling", () => {
  it("converts a daemon Response::Error into an IpcError with its code", async () => {
    invokeMock.mockResolvedValue({
      type: "error",
      code: "not_found",
      message: "no such site",
    });
    await expect(unlink("ghost")).rejects.toMatchObject({
      code: "not_found",
      message: "no such site",
    });
  });

});

describe("IpcError categorisation", () => {
  it("detects an unreachable daemon from the message", () => {
    expect(
      new IpcError("daemon unreachable: /run/user/1000/yerd/yerd.sock").unreachable,
    ).toBe(true);
    expect(new IpcError("daemon is not running").unreachable).toBe(true);
  });

  it("a typed daemon error is not 'unreachable'", () => {
    const e = new IpcError("no such site", "not_found");
    expect(e.code).toBe("not_found");
    expect(e.unreachable).toBe(false);
  });

  it("the explicit unreachable code sets the flag", () => {
    expect(new IpcError("x", "unreachable").unreachable).toBe(true);
  });
});
