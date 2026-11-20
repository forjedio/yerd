import { describe, expect, it } from "vitest";

import {
  commandList,
  needsComposer,
  NODE_TOOL,
  onPathNote,
  splitTools,
  statusText,
  toolDescription,
} from "./tooling";
import type { ToolStatus } from "@/ipc/types";

function tool(id: string, extra: Partial<ToolStatus> = {}): ToolStatus {
  return {
    id,
    display_name: id,
    installed: false,
    version: null,
    binaries: [id],
    ...extra,
  };
}

const ids = (list: ToolStatus[]) => list.map((t) => t.id);

describe("splitTools", () => {
  it.each([
    {
      name: "node installed is dropped",
      tools: [tool(NODE_TOOL, { installed: true }), tool("composer", { installed: true })],
      installed: ["composer"],
      available: [],
    },
    {
      name: "node not installed is dropped",
      tools: [tool(NODE_TOOL), tool("bun")],
      installed: [],
      available: ["bun"],
    },
    {
      name: "daemon order is kept",
      tools: [
        tool("composer", { installed: true }),
        tool("bun"),
        tool("pnpm", { installed: true }),
        tool("laravel"),
        tool("wp-cli", { external: true }),
      ],
      installed: ["composer", "pnpm"],
      available: ["bun", "laravel", "wp-cli"],
    },
    { name: "empty", tools: [], installed: [], available: [] },
  ])("$name", ({ tools, installed, available }) => {
    const split = splitTools(tools);
    expect(ids(split.installed)).toEqual(installed);
    expect(ids(split.available)).toEqual(available);
  });
});

describe("needsComposer", () => {
  it.each([
    { id: "laravel", composer: { installed: true }, expected: false },
    { id: "laravel", composer: { external: true }, expected: true },
    { id: "laravel", composer: null, expected: true },
    { id: "wp-cli", composer: { installed: true }, expected: false },
    { id: "wp-cli", composer: { external: true }, expected: true },
    { id: "wp-cli", composer: null, expected: true },
    { id: "bun", composer: null, expected: false },
    { id: "composer", composer: null, expected: false },
  ])("$id with composer $composer is $expected", ({ id, composer, expected }) => {
    const t = tool(id);
    const tools = composer ? [tool("composer", composer), t] : [t];
    expect(needsComposer(t, tools)).toBe(expected);
  });
});

describe("command copy", () => {
  it.each([
    [[], ""],
    [["composer"], "composer"],
    [["bun", "bunx"], "bun and bunx"],
    [["node", "npm", "npx"], "node, npm and npx"],
  ])("commandList(%j) is %j", (bins, expected) => {
    expect(commandList(bins)).toBe(expected);
  });

  it.each([
    [["composer"], "composer is on your PATH"],
    [["bun", "bunx"], "bun and bunx are on your PATH"],
    [["node", "npm", "npx"], "node, npm and npx are on your PATH"],
  ])("onPathNote(%j) is %j", (bins, expected) => {
    expect(onPathNote(bins)).toBe(expected);
  });
});

describe("toolDescription", () => {
  it.each([
    ["composer", "PHP dependency manager"],
    ["bun", "Fast JavaScript runtime and package manager"],
    ["wp-cli", "The wp command for managing WordPress sites"],
    ["unknown", ""],
  ])("%s", (id, expected) => {
    expect(toolDescription(id)).toBe(expected);
  });
});

describe("statusText", () => {
  it.each([
    ["3/5 [===============>-----------]  60%", "3/5 · 60%"],
    ["0/5 [>---------------------------]   0%", "0/5 · 0%"],
    ["  - Downloading illuminate/support (v13.43.0)", "Downloading illuminate/support (v13.43.0)"],
    ["Installing Laravel Installer…", "Installing Laravel Installer…"],
    ["   ", null],
    ["[====]", null],
  ])("%j becomes %j", (line, expected) => {
    expect(statusText(line)).toBe(expected);
  });
});
