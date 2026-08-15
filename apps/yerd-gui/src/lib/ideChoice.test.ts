import { describe, expect, it } from "vitest";

import { resolveIde, type IdeChoice } from "./ideChoice";
import type { IdeOption } from "@/ipc/types";

const phpstorm: IdeOption = { id: "phpstorm", label: "PhpStorm" };
const zed: IdeOption = { id: "zed", label: "Zed" };
const detected = [phpstorm, zed];

describe("resolveIde", () => {
  const cases: {
    name: string;
    override: string | null;
    global: string | null;
    detected: IdeOption[];
    expected: IdeChoice;
  }[] = [
    {
      name: "honours a per-site override over everything else",
      override: "zed",
      global: "phpstorm",
      detected,
      expected: { kind: "ide", id: "zed", label: "Zed" },
    },
    {
      name: "honours a per-site system override",
      override: "system",
      global: "phpstorm",
      detected,
      expected: { kind: "system", label: "Open folder" },
    },
    {
      name: "falls through an override naming an editor that is not installed",
      override: "sublime",
      global: "zed",
      detected,
      expected: { kind: "ide", id: "zed", label: "Zed" },
    },
    {
      name: "uses the global preference when there is no override",
      override: null,
      global: "zed",
      detected,
      expected: { kind: "ide", id: "zed", label: "Zed" },
    },
    {
      name: "honours a global system preference",
      override: null,
      global: "system",
      detected,
      expected: { kind: "system", label: "Open folder" },
    },
    {
      name: "falls through a global preference naming an editor that is not installed",
      override: null,
      global: "sublime",
      detected,
      expected: { kind: "ide", id: "phpstorm", label: "PhpStorm" },
    },
    {
      name: "auto-detects the best-ranked editor when nothing is stored",
      override: null,
      global: null,
      detected,
      expected: { kind: "ide", id: "phpstorm", label: "PhpStorm" },
    },
    {
      name: "falls back to the system default when nothing is detected",
      override: null,
      global: null,
      detected: [],
      expected: { kind: "system", label: "Open folder" },
    },
    {
      name: "falls back to the system default when every stored id is uninstalled",
      override: "sublime",
      global: "vscode",
      detected: [],
      expected: { kind: "system", label: "Open folder" },
    },
  ];

  for (const c of cases) {
    it(c.name, () => {
      expect(resolveIde(c.override, c.global, c.detected)).toEqual(c.expected);
    });
  }
});
