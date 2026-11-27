import { describe, expect, it } from "vitest";

import { formatDuration, formatElapsed, laravelCommand } from "./createSite";
import type { LaravelOptions } from "@/ipc/types";

function opts(overrides: Partial<LaravelOptions> = {}): LaravelOptions {
  return {
    starter_kit: "none",
    auth: "laravel",
    livewire_class_components: false,
    teams: false,
    testing: "pest",
    database: "sqlite",
    js: "skip",
    git: false,
    boost: false,
    ...overrides,
  };
}

describe("laravelCommand", () => {
  it.each<[string, string, LaravelOptions, string]>([
    ["the daemon's minimal case", "demo", opts(), "laravel new demo --pest --database=sqlite"],
    [
      "the daemon's react/pest/sqlite/npm/git case",
      "shop",
      opts({ starter_kit: "react", js: "npm", git: true }),
      "laravel new shop --react --pest --database=sqlite --npm --git",
    ],
    [
      "the daemon's livewire/workos/teams/phpunit/pgsql/bun/boost case",
      "crm",
      opts({
        starter_kit: "livewire",
        auth: "work_os",
        livewire_class_components: true,
        teams: true,
        testing: "php_unit",
        database: "pgsql",
        js: "bun",
        boost: true,
      }),
      "laravel new crm --livewire --workos --livewire-class-components --teams --phpunit --database=pgsql --bun --boost",
    ],
    [
      "the daemon's no-auth case",
      "x",
      opts({
        starter_kit: "react",
        auth: "none",
        teams: true,
        livewire_class_components: true,
      }),
      "laravel new x --react --no-authentication --pest --database=sqlite",
    ],
    [
      "the daemon's no-auth without an official kit case",
      "x",
      opts({ starter_kit: { community: "acme/kit" }, auth: "none", teams: true }),
      "laravel new x --using=acme/kit --teams --pest --database=sqlite",
    ],
    [
      "a community kit",
      "x",
      opts({ starter_kit: { community: "acme/kit" }, js: "pnpm" }),
      "laravel new x --using=acme/kit --pest --database=sqlite --pnpm",
    ],
    [
      "skipped frontend dependencies",
      "invoicer",
      opts({ starter_kit: "vue", database: "mysql", js: "skip" }),
      "laravel new invoicer --vue --pest --database=mysql",
    ],
  ])("renders %s", (_why, name, o, expected) => {
    expect(laravelCommand(name, o)).toBe(expected);
  });
});

describe("formatElapsed", () => {
  it.each([
    [0, "0:00"],
    [42_400, "0:42"],
    [72_000, "1:12"],
    [725_000, "12:05"],
    [-5, "0:00"],
  ])("formats %d ms as %s", (ms, expected) => {
    expect(formatElapsed(ms)).toBe(expected);
  });
});

describe("formatDuration", () => {
  it.each([
    [0, "0s"],
    [41_600, "42s"],
    [72_000, "1m 12s"],
    [180_000, "3m"],
  ])("formats %d ms as %s", (ms, expected) => {
    expect(formatDuration(ms)).toBe(expected);
  });
});
