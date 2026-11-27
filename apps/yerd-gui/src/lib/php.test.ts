import { describe, expect, it } from "vitest";

import type { PhpPoolStatus, PhpVersionsResponse } from "@/ipc/types";
import {
  bannerCopy,
  configuredCount,
  installChoices,
  listVersions,
  parseInstallLine,
  statusLine,
  updateLabel,
  xdebugCell,
} from "./php";

function pool(over: Partial<PhpPoolStatus> = {}): PhpPoolStatus {
  return {
    version: "8.5",
    installed_patch: "8.5.1",
    state: "running",
    pid: 1,
    listen: null,
    rss_bytes: null,
    update_available: null,
    ...over,
  };
}

function response(over: Partial<PhpVersionsResponse> = {}): PhpVersionsResponse {
  return { type: "php_versions", installed: ["8.4"], default: "8.4", ...over };
}

const EXT = { name: "scrypt", path: "/x/scrypt.so", zend: false, present: true };

describe("listVersions", () => {
  it.each([
    ["installed only", ["8.3", "8.4"], [], ["8.4", "8.3"]],
    ["the union, deduplicated", ["8.4"], ["8.4", "8.2"], ["8.4", "8.2"]],
    ["8.10 after 8.9", ["8.9", "8.10", "7.4"], [], ["8.10", "8.9", "7.4"]],
    ["nothing", [], [], []],
  ])("orders %s newest first", (_name, installed, exts, expected) => {
    expect(listVersions(installed, exts)).toEqual(expected);
  });
});

describe("configuredCount", () => {
  const data = response({
    installed: ["8.4"],
    version_settings: { "8.4": { memory_limit: "1G", display_errors: "" } },
    directives: { "8.4": { "opcache.enable": "1" } },
    pool: { "8.4": { max_children: "32" } },
  });

  it("adds overrides, directives, pool and extensions for an installed version", () => {
    expect(configuredCount(data, { "8.4": [EXT] }, "8.4", ["8.4"])).toBe(4);
  });

  it("counts only extensions for an uninstalled version", () => {
    expect(configuredCount(data, { "8.2": [EXT, EXT] }, "8.2", ["8.4"])).toBe(2);
  });

  it("is zero with nothing loaded", () => {
    expect(configuredCount(null, null, "8.4", ["8.4"])).toBe(0);
  });
});

describe("statusLine", () => {
  it.each([
    ["running row", pool(), "row", "8.5.1 · running"],
    ["stopped row", pool({ state: "stopped" }), "row", "8.5.1 · idle"],
    ["failed row", pool({ state: "failed" }), "row", "8.5.1 · failed"],
    ["unreported row", undefined, "row", "idle"],
    ["running inspector", pool(), "inspector", "8.5.1 · FPM pool running"],
    ["no patch inspector", pool({ installed_patch: null }), "inspector", "FPM pool running"],
  ] as const)("%s", (_name, p, kind, expected) => {
    expect(statusLine(p, kind)).toBe(expected);
  });
});

describe("updateLabel", () => {
  const update = { version: "8.4", installed: "8.4.14", latest: "8.4.15" };

  it("shows an available update first, even for a legacy version", () => {
    expect(updateLabel(update, false)).toEqual({
      kind: "update",
      text: "↑ 8.4.15",
      suffix: "update to 8.4.15 available",
    });
    expect(updateLabel(update, true).kind).toBe("update");
  });

  it("marks a legacy version without an update as EOL", () => {
    expect(updateLabel(undefined, true)).toEqual({ kind: "eol", text: "EOL", suffix: "out of support" });
  });

  it("is up to date otherwise", () => {
    expect(updateLabel(undefined, false)).toEqual({
      kind: "current",
      text: "up to date",
      suffix: "up to date",
    });
  });
});

describe("xdebugCell", () => {
  const data = response({
    xdebug: {
      "8.4": { enabled: true, modes: ["debug"], start: "always" },
      "8.1": { enabled: true, modes: ["debug"], start: "always" },
      "8.3": { enabled: false, modes: ["debug"], start: "trigger" },
    },
    xdebug_versions: ["8.3", "8.4"],
  });

  it.each([
    ["8.4", "on"],
    ["8.1", "on"],
    ["8.3", "off"],
    ["7.4", "unavailable"],
  ])("PHP %s is %s", (version, expected) => {
    expect(xdebugCell(version, data)).toBe(expected);
  });

  it("is unavailable before the list loads", () => {
    expect(xdebugCell("8.4", null)).toBe("unavailable");
  });
});

describe("bannerCopy", () => {
  it("is null without updates", () => {
    expect(bannerCopy([])).toBeNull();
  });

  it("updates the one version", () => {
    expect(bannerCopy([{ version: "8.4", installed: "8.4.14", latest: "8.4.15" }])).toEqual({
      title: "1 update available",
      detail: "PHP 8.4.14 → 8.4.15",
      action: "Update",
      target: "8.4",
    });
  });

  it("updates all of several, listed newest first", () => {
    expect(
      bannerCopy([
        { version: "8.3", installed: "8.3.1", latest: "8.3.2" },
        { version: "8.4", installed: "8.4.14", latest: "8.4.15" },
      ]),
    ).toEqual({
      title: "2 updates available",
      detail: "PHP 8.4, 8.3",
      action: "Update all",
      target: null,
    });
  });
});

describe("installChoices", () => {
  it("sorts each group newest first and marks installed rows", () => {
    const c = installChoices(["8.2", "8.5", "8.4"], ["7.4", "8.1"], ["8.5", "7.4"]);
    expect(c.supported).toEqual([
      { version: "8.5", installed: true },
      { version: "8.4", installed: false },
      { version: "8.2", installed: false },
    ]);
    expect(c.legacy).toEqual([
      { version: "8.1", installed: false },
      { version: "7.4", installed: true },
    ]);
  });

  it.each([
    ["the newest uninstalled supported", ["8.4", "8.5"], ["8.1"], ["8.5"], "8.4"],
    ["the newest uninstalled legacy when supported are all in", ["8.5"], ["8.0", "8.1"], ["8.5"], "8.1"],
    ["nothing when everything is installed", ["8.5"], ["8.1"], ["8.5", "8.1"], ""],
  ])("picks %s", (_name, available, legacy, installed, expected) => {
    expect(installChoices(available, legacy, installed).defaultChoice).toBe(expected);
  });
});

describe("parseInstallLine", () => {
  it("reads a line with a total", () => {
    expect(parseInstallLine("Downloading CLI: 18.0 MB / 42.0 MB")).toEqual({
      file: "CLI",
      doneMb: "18.0",
      totalMb: "42.0",
      percent: 42,
    });
  });

  it("reads a line without a total", () => {
    expect(parseInstallLine("Downloading FPM: 4.0 MB")).toEqual({
      file: "FPM",
      doneMb: "4.0",
      totalMb: null,
      percent: null,
    });
  });

  it("caps a finished download at 100 and leaves a zero total without a percent", () => {
    expect(parseInstallLine("Downloading CLI: 43.0 MB / 42.0 MB")?.percent).toBe(100);
    expect(parseInstallLine("Downloading CLI: 0.0 MB / 0.0 MB")?.percent).toBeNull();
  });

  it.each(["Found PHP 8.2.29 — downloading…", "Resolving PHP 8.2…", "garbage", ""])(
    "ignores %j",
    (line) => {
      expect(parseInstallLine(line)).toBeNull();
    },
  );
});
