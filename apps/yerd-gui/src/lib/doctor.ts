import type { Diagnosis, ElevateTarget, Severity } from "@/ipc/types";
import { type PrivilegeItem, privilegeStatus } from "@/lib/elevation";

/** Pure derivations for the Doctor screen. No I/O. */

/** A finding with its stable selection key. */
export interface FindingItem {
  key: string;
  finding: Diagnosis;
}

/** One list group: Problems (fail), Warnings (warn) or Info (ok). */
export interface FindingGroup {
  severity: Severity;
  label: string;
  items: FindingItem[];
}

const GROUP_ORDER: { severity: Severity; label: string }[] = [
  { severity: "fail", label: "Problems" },
  { severity: "warn", label: "Warnings" },
  { severity: "ok", label: "Info" },
];

/**
 * Groups the daemon's findings by severity, keeping daemon order within a group.
 * The synthetic `all_good` finding is dropped (the all-clear state replaces it)
 * and empty groups are left out. Keys are `code:title`, since codes repeat (one
 * `service_failed` per service); a repeated title gets a `#n` suffix.
 */
export function findingGroups(ds: readonly Diagnosis[]): FindingGroup[] {
  const seen = new Map<string, number>();
  const keyed = ds
    .filter((d) => d.code !== "all_good")
    .map((finding) => {
      const base = `${finding.code}:${finding.title}`;
      const n = seen.get(base) ?? 0;
      seen.set(base, n + 1);
      return { key: n === 0 ? base : `${base}#${n}`, finding };
    });
  return GROUP_ORDER.map(({ severity, label }) => ({
    severity,
    label,
    items: keyed.filter((i) => i.finding.severity === severity),
  })).filter((g) => g.items.length > 0);
}

/** The inspector chip for a finding's severity. */
export const SEVERITY_CHIP: Record<Severity, string> = {
  fail: "Problem",
  warn: "Warning",
  ok: "Info",
};

/** Whether any finding is a warning or failure, which is when Run safe fixes applies. */
export function hasActionable(ds: readonly Diagnosis[]): boolean {
  return ds.some((d) => d.severity === "warn" || d.severity === "fail");
}

/** The first problem or warning to select, or `null` so the all-clear state shows. */
export function defaultSelection(groups: readonly FindingGroup[]): string | null {
  const first = groups.find((g) => g.severity !== "ok");
  return first?.items[0]?.key ?? null;
}

/** The list and selection key of a system permission row. */
export function permissionKey(target: ElevateTarget): string {
  return `perm:${target}`;
}

/** Copy for a system permission's row and inspector. */
export interface PermissionCopy {
  /** Row subtitle, e.g. "Not trusted". */
  summary: string;
  /** Inspector chip, the summary up to any " - " explanation. */
  chip: string;
  description: string;
  cardTitle: string;
  cardBody: string;
}

const DESCRIPTION: Record<ElevateTarget, string> = {
  trust: "HTTPS .test sites open in your browser without certificate warnings.",
  resolver: "Every .test name resolves to Yerd, so sites open by their domain.",
  ports: "Sites are served on the standard ports 80 and 443, so URLs need no port number.",
};

function card(item: PrivilegeItem): { cardTitle: string; cardBody: string } {
  if (item.value === true) {
    return {
      cardTitle: "Granted",
      cardBody: item.unelevatable
        ? "Yerd set this up for you. Revert removes it if you no longer want it."
        : "This permission is in place. Nothing to do.",
    };
  }
  if (item.value === null) {
    return {
      cardTitle: "Status unknown",
      cardBody: "Yerd couldn't read this setting. Fixing it again is safe.",
    };
  }
  if (!item.fixable) {
    return { cardTitle: "Not available yet", cardBody: "Elevation can't fix this yet." };
  }
  return {
    cardTitle: "Needs your password",
    cardBody: "Fix asks the operating system for your password once, then applies this setting.",
  };
}

/** The row subtitle, chip, description and status card for a system permission. */
export function permissionCopy(item: PrivilegeItem): PermissionCopy {
  const status = privilegeStatus(item);
  const summary = status.charAt(0).toUpperCase() + status.slice(1);
  return {
    summary,
    chip: summary.split(" - ")[0] ?? summary,
    description: DESCRIPTION[item.target],
    ...card(item),
  };
}
