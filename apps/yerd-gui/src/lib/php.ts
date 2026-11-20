/**
 * Pure derivations for the PHP screen: the version list's order and labels, the
 * update banner, the install dialog's choices and the install-progress line.
 * No I/O; the view and its components feed these the daemon's replies.
 */

import type { PhpExtInfo, PhpPoolStatus, PhpUpdate, PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import { overrideCount } from "@/lib/phpSettings";
import { comparePhpVersions } from "@/lib/phpVersion";
import { poolStateLabel } from "@/lib/utils";

/**
 * Every version the list shows, newest first: the installed ones plus any that
 * still hold registered extensions after an uninstall, since the daemon still
 * lets those be removed.
 */
export function listVersions(installed: PhpVersion[], extensionVersions: PhpVersion[]): PhpVersion[] {
  const seen = new Set<PhpVersion>(installed);
  for (const v of extensionVersions) seen.add(v);
  return [...seen].sort((a, b) => comparePhpVersions(b, a));
}

/**
 * How much a version has configured, for the row's "customised" tag. An
 * uninstalled version counts only its extensions: the daemon refuses to change
 * its overrides and directives, so the inspector hides them.
 */
export function configuredCount(
  data: PhpVersionsResponse | null,
  extensions: Record<PhpVersion, PhpExtInfo[]> | null,
  version: PhpVersion,
  installed: PhpVersion[],
): number {
  const exts = extensions?.[version]?.length ?? 0;
  if (!installed.includes(version)) return exts;
  return (
    overrideCount(data?.version_settings?.[version] ?? {}) +
    Object.keys(data?.directives?.[version] ?? {}).length +
    Object.keys(data?.pool?.[version] ?? {}).length +
    exts
  );
}

/**
 * A version's pool status line: "8.5.1 · running" in a list row, "8.5.1 · FPM
 * pool running" in the inspector. The patch is left out until the status poll
 * reports it.
 */
export function statusLine(pool: PhpPoolStatus | undefined, kind: "row" | "inspector"): string {
  const state = poolStateLabel(pool?.state);
  const word = kind === "inspector" ? `FPM pool ${state}` : state;
  return pool?.installed_patch ? `${pool.installed_patch} · ${word}` : word;
}

/** A version's update state, as a chip or faint text and the inspector's status suffix. */
export interface UpdateLabel {
  kind: "update" | "eol" | "current";
  text: string;
  suffix: string;
}

/** The update cell for a version: an available patch, out of support (legacy), or up to date. */
export function updateLabel(update: PhpUpdate | undefined, legacy: boolean): UpdateLabel {
  if (update) {
    return {
      kind: "update",
      text: `↑ ${update.latest}`,
      suffix: `update to ${update.latest} available`,
    };
  }
  if (legacy) return { kind: "eol", text: "EOL", suffix: "out of support" };
  return { kind: "current", text: "up to date", suffix: "up to date" };
}

/** The table's XDEBUG cell: on, off, or unavailable when no build exists and it isn't on. */
export function xdebugCell(
  version: PhpVersion,
  data: PhpVersionsResponse | null,
): "on" | "off" | "unavailable" {
  if (data?.xdebug?.[version]?.enabled) return "on";
  return (data?.xdebug_versions ?? []).includes(version) ? "off" : "unavailable";
}

/** The update banner's copy and the version its button updates (`null` = all). */
export interface BannerCopy {
  title: string;
  detail: string;
  action: "Update" | "Update all";
  target: PhpVersion | null;
}

/** The update banner for the pending updates, or `null` when there are none. */
export function bannerCopy(updates: PhpUpdate[]): BannerCopy | null {
  const [first] = updates;
  if (!first) return null;
  if (updates.length === 1) {
    return {
      title: "1 update available",
      detail: `PHP ${first.installed} → ${first.latest}`,
      action: "Update",
      target: first.version,
    };
  }
  const versions = updates.map((u) => u.version).sort((a, b) => comparePhpVersions(b, a));
  return {
    title: `${updates.length} updates available`,
    detail: `PHP ${versions.join(", ")}`,
    action: "Update all",
    target: null,
  };
}

/** One row of the install dialog. */
export interface InstallChoice {
  version: PhpVersion;
  installed: boolean;
}

/** The install dialog's two groups and the row picked when it opens. */
export interface InstallChoices {
  supported: InstallChoice[];
  legacy: InstallChoice[];
  defaultChoice: PhpVersion | "";
}

/**
 * Group the installable versions for the install dialog, newest first, marking
 * the installed ones. The initial pick is the newest uninstalled supported
 * version, else the newest uninstalled legacy one, else nothing.
 */
export function installChoices(
  available: PhpVersion[],
  legacy: PhpVersion[],
  installed: PhpVersion[],
): InstallChoices {
  const have = new Set(installed);
  const group = (vs: PhpVersion[]): InstallChoice[] =>
    [...new Set(vs)]
      .sort((a, b) => comparePhpVersions(b, a))
      .map((version) => ({ version, installed: have.has(version) }));
  const supported = group(available);
  const legacyGroup = group(legacy);
  const pick =
    supported.find((c) => !c.installed) ?? legacyGroup.find((c) => !c.installed) ?? null;
  return { supported, legacy: legacyGroup, defaultChoice: pick?.version ?? "" };
}

/** A parsed `Downloading {file}: X MB[ / Y MB]` progress line. */
export interface InstallProgress {
  file: string;
  doneMb: string;
  totalMb: string | null;
  percent: number | null;
}

const DOWNLOAD_LINE = /^Downloading (.+?): (\d+(?:\.\d+)?) MB(?: \/ (\d+(?:\.\d+)?) MB)?$/;

/**
 * Parse the daemon's per-file download line, or `null` for any other line. The
 * percent is floored and capped at 100, and absent without a known total.
 */
export function parseInstallLine(line: string): InstallProgress | null {
  const m = DOWNLOAD_LINE.exec(line.trim());
  if (!m) return null;
  const [, file = "", doneMb = "0", totalMb] = m;
  const done = Number(doneMb);
  const total = totalMb === undefined ? null : Number(totalMb);
  const percent = total && total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : null;
  return { file, doneMb, totalMb: totalMb ?? null, percent };
}
