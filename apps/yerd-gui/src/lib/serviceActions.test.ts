import { describe, expect, it } from "vitest";

import { canStartService, canStopService, isPerSiteService } from "./serviceActions";
import type { ServiceStatus } from "@/ipc/types";

function svc(over: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "redis",
    display_name: "Redis",
    installed_versions: ["7"],
    selected_version: "7",
    state: "stopped",
    pid: null,
    listen: null,
    port: 6379,
    enabled: true,
    supports_databases: false,
    ...over,
  };
}

describe("isPerSiteService", () => {
  it("is true when site is set", () => {
    expect(isPerSiteService(svc({ site: "blog", installed_versions: [] }))).toBe(true);
  });

  it("is false for single-instance engines", () => {
    expect(isPerSiteService(svc())).toBe(false);
  });
});

describe("canStartService", () => {
  it("allows start when installed and not running", () => {
    expect(canStartService(svc({ state: "stopped" }))).toBe(true);
  });

  it("blocks start when already running", () => {
    expect(canStartService(svc({ state: "running" }))).toBe(false);
  });

  it("allows per-site instances with no installed versions", () => {
    expect(
      canStartService(
        svc({
          service: "reverb:blog",
          display_name: "Reverb",
          installed_versions: [],
          selected_version: null,
          site: "blog",
          state: "stopped",
        }),
      ),
    ).toBe(true);
  });

  it("blocks uninstalled single-instance engines", () => {
    expect(canStartService(svc({ installed_versions: [], selected_version: null }))).toBe(false);
  });
});

describe("canStopService", () => {
  it("allows stop when running or failed", () => {
    expect(canStopService(svc({ state: "running" }))).toBe(true);
    expect(canStopService(svc({ state: "failed" }))).toBe(true);
  });

  it("blocks stop when stopped", () => {
    expect(canStopService(svc({ state: "stopped" }))).toBe(false);
  });
});
