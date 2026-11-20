import { describe, expect, it } from "vitest";

import {
  clearedLabel,
  ltsLabel,
  majorOf,
  offDefaultSites,
  releaseTags,
  sourceLabel,
  usedByCounts,
  usedByLabel,
} from "./node";
import type { MajorLine, NodeRelease, NodeSiteEntry, NodeSource } from "@/ipc/types";

function release(version: string): NodeRelease {
  return { version, lts: false, default: false, uninstall_blocked: false };
}

function line(major: number, extra: Partial<MajorLine> = {}): MajorLine {
  return { major, latest: `${major}.1.0`, update_available: false, ...extra };
}

const index: MajorLine[] = [
  line(25, { eol: false }),
  line(24, { codename: "Krypton", eol: false }),
  line(23, { eol: true }),
  line(22, { codename: "Jod", eol: false }),
  line(16, { codename: "Gallium", eol: true }),
];

describe("labels", () => {
  it.each([
    ["22.12.0", "22"],
    ["9", "9"],
    ["", ""],
  ])("majorOf(%j) is %j", (v, expected) => {
    expect(majorOf(v)).toBe(expected);
  });

  it.each([
    ["Jod", "LTS Jod"],
    [undefined, "LTS"],
  ])("ltsLabel(%j) is %j", (codename, expected) => {
    expect(ltsLabel(codename)).toBe(expected);
  });

  it.each([
    [1, "1 site"],
    [2, "2 sites"],
    [41, "41 sites"],
    [0, "—"],
    [undefined, "—"],
  ])("usedByLabel(%j) is %j", (count, expected) => {
    expect(usedByLabel(count)).toBe(expected);
  });

  it.each<[NodeSource | undefined, string]>([
    ["nvmrc", ".nvmrc"],
    ["node-version", ".node-version"],
    ["site", "saved"],
    ["default", "default"],
    [undefined, ""],
  ])("sourceLabel(%j) is %j", (source, expected) => {
    expect(sourceLabel(source)).toBe(expected);
  });

  it.each([
    [{ root: "/srv/blog", site: "blog" }, "blog.test"],
    [{ root: "/srv/old-app" }, "/srv/old-app"],
  ])("clearedLabel(%j) is %j", (ref, expected) => {
    expect(clearedLabel(ref, "test")).toBe(expected);
  });
});

describe("usedByCounts", () => {
  it("counts sites by their resolved version and skips unresolved ones", () => {
    const entries: NodeSiteEntry[] = [
      { site: "a", effective: "22.9.0", source: "default" },
      { site: "b", effective: "22.9.0", source: "nvmrc" },
      { site: "c", effective: "20.18.0", source: "site" },
      { site: "d", missing: "18", error: "not installed" },
    ];
    expect(usedByCounts(entries)).toEqual({ "22.9.0": 2, "20.18.0": 1 });
  });

  it("is empty without entries", () => {
    expect(usedByCounts(null)).toEqual({});
    expect(usedByCounts([])).toEqual({});
  });
});

describe("releaseTags", () => {
  it.each<[string, string, MajorLine[] | null, { current: boolean; eol: boolean }]>([
    ["no index loaded", "25.1.0", null, { current: false, eol: false }],
    ["line not listed", "21.0.0", index, { current: false, eol: false }],
    ["newest non-LTS line", "25.1.0", index, { current: true, eol: false }],
    ["LTS line", "24.1.0", index, { current: false, eol: false }],
    ["older non-LTS EOL line", "23.1.0", index, { current: false, eol: true }],
    ["EOL LTS line", "16.20.2", index, { current: false, eol: true }],
    [
      "newest line with unknown EOL status",
      "25.1.0",
      [line(25), line(24, { codename: "Krypton" })],
      { current: false, eol: false },
    ],
  ])("%s", (_name, version, majors, expected) => {
    expect(releaseTags(release(version), majors)).toEqual(expected);
  });
});

describe("offDefaultSites", () => {
  const entries: NodeSiteEntry[] = [
    { site: "shop", effective: "22.9.0", source: "nvmrc" },
    { site: "home", effective: "22.9.0", source: "default" },
    { site: "blog", effective: "20.18.0", source: "site", preference: "20" },
    {
      site: "legacy",
      project_file: { kind: "nvmrc", selector: "18" },
      missing: "18",
      error: "Node 18 is not installed",
    },
    { site: "wp", preference: "lts/hydrogen", missing: "lts/hydrogen" },
    { site: "broken", error: "unreadable .nvmrc" },
  ];

  it("keeps off-default and unresolved sites in daemon order", () => {
    expect(offDefaultSites(entries, "22.9.0", "test")).toEqual([
      {
        site: "blog",
        domain: "blog.test",
        source: "saved",
        chip: "Node 20",
        unresolved: false,
      },
      {
        site: "legacy",
        domain: "legacy.test",
        source: ".nvmrc",
        chip: "Node 18 missing",
        unresolved: true,
        title: "Node 18 is not installed",
      },
      {
        site: "wp",
        domain: "wp.test",
        source: "saved",
        chip: "Node lts/hydrogen missing",
        unresolved: true,
        title: undefined,
      },
      {
        site: "broken",
        domain: "broken.test",
        source: "",
        chip: "Unresolved",
        unresolved: true,
        title: "unreadable .nvmrc",
      },
    ]);
  });

  it("lists every resolved site when no default is installed", () => {
    const rows = offDefaultSites(entries.slice(0, 3), undefined, "localhost");
    expect(rows.map((r) => r.domain)).toEqual(["shop.localhost", "home.localhost", "blog.localhost"]);
    expect(rows.map((r) => r.source)).toEqual([".nvmrc", "default", "saved"]);
  });

  it("is empty when every site is on the default", () => {
    expect(offDefaultSites(entries.slice(0, 2), "22.9.0", "test")).toEqual([]);
  });
});
