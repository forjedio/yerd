import type { ToolStatus } from "@/ipc/types";
import { joinWithAnd } from "@/lib/utils";

/**
 * Pure derivations for the Tooling screen. No I/O; the input is the daemon's
 * `list_tools` reply.
 */

/** Node has its own page (versions, defaults, updates), so Tooling never lists it. */
export const NODE_TOOL = "node";

const DESCRIPTIONS: Record<string, string> = {
  composer: "PHP dependency manager",
  pnpm: "Fast, disk-efficient JavaScript package manager. Needs Node to run scripts.",
  laravel: "Scaffolds new Laravel apps — used by New site",
  bun: "Fast JavaScript runtime and package manager",
  "wp-cli": "The wp command for managing WordPress sites",
  cloudflared: "Cloudflare Tunnel connector - used by Share",
};

/** The short description shown for a tool, keyed by wire id; empty for unknown ids. */
export function toolDescription(id: string): string {
  return DESCRIPTIONS[id] ?? "";
}

/** Splits the tool list into installed and available, dropping Node and keeping daemon order. */
export function splitTools(tools: ToolStatus[]): {
  installed: ToolStatus[];
  available: ToolStatus[];
} {
  const listed = tools.filter((t) => t.id !== NODE_TOOL);
  return {
    installed: listed.filter((t) => t.installed),
    available: listed.filter((t) => !t.installed),
  };
}

/**
 * True when `t` is the Laravel installer or WP-CLI and Yerd's own Composer is
 * not installed. Managed-only: these installers are built with Yerd's Composer
 * (an external Composer can't build them), so `external` is ignored.
 */
export function needsComposer(t: ToolStatus, tools: ToolStatus[]): boolean {
  if (t.id !== "laravel" && t.id !== "wp-cli") return false;
  return !tools.some((c) => c.id === "composer" && c.installed);
}

/** Joins command names for prose: `a`, `a and b`, `a, b and c`. */
export function commandList(bins: string[]): string {
  return joinWithAnd(bins);
}

/** The success toast detail, e.g. "bun and bunx are on your PATH". */
export function onPathNote(bins: string[]): string {
  return `${commandList(bins)} ${bins.length > 1 ? "are" : "is"} on your PATH`;
}

const ASCII_BAR = /\[[=>\-.#\s]*\]/g;

/**
 * A log line made readable for one-line status text (the sidebar operations
 * indicator and an installing card): ASCII progress bars such as Composer's
 * `3/5 [====>---]  60%` become `3/5 · 60%`, and list markers are dropped.
 * Returns null when nothing readable is left.
 */
export function statusText(line: string): string | null {
  const text = line
    .replace(ASCII_BAR, "·")
    .replace(/^\s*[-*]\s+/, "")
    .replace(/\s+/g, " ")
    .trim();
  return text && text !== "·" ? text : null;
}
