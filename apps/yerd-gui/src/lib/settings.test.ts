import { describe, expect, it } from "vitest";

import type { AutostartState, StatusReport, UpdateStatusResponse } from "@/ipc/types";
import { portFromAddr, settingsBanners, updateHeadline } from "./settings";

const NOW = 1_000_000;

function upd(over: Partial<UpdateStatusResponse> = {}): UpdateStatusResponse {
  return {
    type: "update_status",
    current: "2.1.0-rc.1",
    latest_stable: "2.0.4",
    latest_edge: "2.1.0",
    channel: "edge",
    available: false,
    target: null,
    ahead_of_stable: false,
    source: "live",
    checked_at_epoch: NOW - 240,
    ...over,
  };
}

function autostart(over: Partial<AutostartState> = {}): AutostartState {
  return {
    daemon: true,
    daemonSupported: true,
    gui: true,
    guiMinimized: false,
    daemonPendingApproval: false,
    guiPendingApproval: false,
    ...over,
  };
}

function report(over: Partial<StatusReport> = {}): StatusReport {
  return { web_unbound: null, dns_unbound: null, ...over } as unknown as StatusReport;
}

const TOOLTIP = "Current 2.1.0-rc.1 · stable 2.0.4 · edge 2.1.0";

describe("portFromAddr", () => {
  it.each([
    ["127.0.0.1:1053", 1053],
    ["[::1]:53", 53],
    ["", 0],
    [undefined, 0],
    ["garbage", 0],
    ["host:abc", 0],
  ] as const)("%s -> %d", (addr, port) => {
    expect(portFromAddr(addr)).toBe(port);
  });
});

describe("updateHeadline", () => {
  it.each([
    [
      "no status, running",
      null,
      { running: true, applying: false },
      { tone: "none", title: "Not checked yet.", detail: "", tooltip: null },
    ],
    [
      "no status, daemon down",
      null,
      { running: false, applying: false },
      { tone: "none", title: "Start the daemon to check for updates.", detail: "", tooltip: null },
    ],
    [
      "never checked",
      upd({ checked_at_epoch: undefined, latest_stable: null, latest_edge: null }),
      { running: true, applying: false },
      {
        tone: "none",
        title: "Not checked yet.",
        detail: "",
        tooltip: "Current 2.1.0-rc.1 · stable - · edge -",
      },
    ],
    [
      "up to date",
      upd(),
      { running: true, applying: false, now: NOW },
      { tone: "ok", title: "Up to date", detail: "Last checked 4 minutes ago.", tooltip: TOOLTIP },
    ],
    [
      "up to date without a timestamp",
      upd({ checked_at_epoch: undefined }),
      { running: true, applying: false, now: NOW },
      { tone: "ok", title: "Up to date", detail: "", tooltip: TOOLTIP },
    ],
    [
      "ahead of stable",
      upd({ ahead_of_stable: true }),
      { running: true, applying: false, now: NOW },
      {
        tone: "ok",
        title: "Up to date",
        detail: "On a pre-release ahead of stable. Last checked 4 minutes ago.",
        tooltip: TOOLTIP,
      },
    ],
    [
      "ahead of stable without a timestamp",
      upd({ ahead_of_stable: true, checked_at_epoch: undefined }),
      { running: true, applying: false, now: NOW },
      { tone: "ok", title: "Up to date", detail: "On a pre-release ahead of stable.", tooltip: TOOLTIP },
    ],
    [
      "available",
      upd({ available: true, target: "2.1.0" }),
      { running: true, applying: false, now: NOW },
      {
        tone: "available",
        title: "Yerd 2.1.0 is available",
        detail: "You're on 2.1.0-rc.1. Yerd restarts to finish.",
        tooltip: TOOLTIP,
      },
    ],
    [
      "applying",
      upd({ available: true, target: "2.1.0" }),
      { running: true, applying: true, now: NOW },
      {
        tone: "available",
        title: "Yerd 2.1.0 is available",
        detail: "Updating to 2.1.0. Yerd will restart…",
        tooltip: TOOLTIP,
      },
    ],
  ] as const)("%s", (_name, status, opts, expected) => {
    expect(updateHeadline(status, opts)).toEqual(expected);
  });
});

describe("settingsBanners", () => {
  it("is empty when nothing needs attention", () => {
    expect(settingsBanners(autostart(), report())).toEqual([]);
    expect(settingsBanners(null, null)).toEqual([]);
  });

  it("lists all four in order", () => {
    const banners = settingsBanners(
      autostart({ daemonPendingApproval: true, guiPendingApproval: true }),
      report({ web_unbound: { http: 8080, https: 8443 }, dns_unbound: 1053 }),
    );
    expect(banners.map((b) => [b.id, b.tone, b.loginItems])).toEqual([
      ["daemon-approval", "warning", true],
      ["gui-approval", "warning", true],
      ["web-unbound", "destructive", false],
      ["dns-unbound", "destructive", false],
    ]);
    expect(banners[0]?.title).toBe("Approve the Yerd background daemon");
    expect(banners[2]).toMatchObject({
      title: "Yerd isn't serving any sites",
      body: "Ports 8080/8443 are in use by another process. Pick free HTTP and HTTPS ports and save.",
    });
    expect(banners[3]?.body).toContain("Port 1053 is in use");
  });

  it("shows only the approval banner without a report", () => {
    expect(
      settingsBanners(autostart({ daemonPendingApproval: true }), null).map((b) => b.id),
    ).toEqual(["daemon-approval"]);
  });
});
