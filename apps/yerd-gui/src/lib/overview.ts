import type {
  Contributor,
  GroupsState,
  ReleaseNoteItem,
  ReleaseNotesEntry,
  ReleaseNotesResponse,
  ServiceStatus,
  SiteEntry,
  SiteFramework,
  StatusReport,
} from "@/ipc/types";
import { isInstalled } from "@/lib/services";
import { asciiLower, memberGroup } from "@/lib/sites";
import { humaniseUptime, plural } from "@/lib/utils";

/**
 * Pure derivations behind the Overview screen. Everything here reads the daemon's
 * `StatusReport` (and site list) and decides what to display; nothing fetches or
 * mutates, so the cards stay thin and the rules are table-tested.
 */

/** Ports below this need privilege (or the macOS forwarder) to bind. */
const PRIVILEGED_PORT_CEILING = 1024;

/** The headline state of the status card. */
export type StatusKind = "serving" | "not-serving" | "ready";

/** Tone of one inset health row. */
export type HealthTone = "ok" | "warn" | "bad";

/** One chip of the environment strip's health row. */
export interface HealthRow {
  key: string;
  tone: HealthTone;
  text: string;
  /** In-app destination that fixes the row's problem. */
  link?: { label: string; to: string };
}

/** The status card's big headline. `showTld` appends the mono `.{tld}`. */
export interface SiteHeadline {
  text: string;
  showTld: boolean;
}

/** Keys of the first-run checklist steps, in order. */
export type SetupStepKey = "trust" | "php" | "park";

/** One first-run checklist step and whether it is already done. */
export interface SetupStep {
  key: SetupStepKey;
  done: boolean;
}

/** Display names for the frameworks the daemon recognises (`other` has none). */
export const FRAMEWORK_LABELS: Partial<Record<SiteFramework, string>> = {
  laravel: "Laravel",
  wordpress: "WordPress",
  symfony: "Symfony",
  statamic: "Statamic",
  craft: "Craft",
  drupal: "Drupal",
};

/**
 * Whether 80/443 are effectively served. Fails only when a privileged port fell
 * back to its rootless alternative and no port redirect carries it.
 */
export function portsReady(report: StatusReport): boolean {
  const fellPrivileged =
    (report.http.requested < PRIVILEGED_PORT_CEILING && report.http.fell_back) ||
    (report.https.requested < PRIVILEGED_PORT_CEILING && report.https.fell_back);
  return !fellPrivileged || report.port_redirect === true;
}

/** The status card's state: degraded when the web ports are unbound, else by site count. */
export function statusKind(report: StatusReport, siteCount: number): StatusKind {
  if (report.web_unbound) return "not-serving";
  return siteCount === 0 ? "ready" : "serving";
}

/** The headline for a status kind and site count. */
export function siteHeadline(kind: StatusKind, siteCount: number): SiteHeadline {
  if (kind === "ready") return { text: "No sites yet", showTld: false };
  if (kind === "not-serving") {
    return { text: `0 of ${plural(siteCount, "site", "sites")}`, showTld: true };
  }
  return { text: plural(siteCount, "site", "sites"), showTld: true };
}

/** The mono meta line under the headline, as segments the template joins with ` · `. */
export function metaSegments(report: StatusReport): string[] {
  const out = [
    `yerdd ${report.daemon_version || "unknown"}`,
    `up ${humaniseUptime(report.uptime_secs)}`,
    `pid ${report.daemon_pid}`,
  ];
  const { rows, running } = serviceSummary(report.services ?? []);
  if (rows.length > 0) out.push(`${running} of ${plural(rows.length, "service", "services")} running`);
  if (report.php.length === 0) out.push("no PHP installed");
  return out;
}

const DOCTOR_FIX = { label: "Fix", to: "/doctor" };

/**
 * The environment strip's health chips. Each healthy CA or resolver check is
 * its own `ok` chip; each failing or unknown check gets a `warn` chip linking
 * to Doctor. A web-port or DNS-port bind failure is a `bad` chip linking to
 * Settings (an installed resolver is no comfort while the DNS port is unbound,
 * so its `ok` chip goes then); a privileged-port fallback without a redirect is
 * a `warn` chip linking to Doctor. Healthy ports add nothing.
 */
export function healthRows(report: StatusReport): HealthRow[] {
  const tld = report.tld;
  const healthy: HealthRow[] = [];
  const problems: HealthRow[] = [];

  const ca = report.ca.trusted_system;
  if (ca === true) healthy.push({ key: "trust", tone: "ok", text: "Local CA trusted" });
  else {
    problems.push({
      key: "trust",
      tone: "warn",
      text: ca === false ? "Local CA isn't trusted" : "Local CA trust is unknown",
      link: DOCTOR_FIX,
    });
  }

  const resolver = report.resolver_installed;
  const dnsUnbound = report.dns_unbound;
  if (dnsUnbound != null) {
    problems.push({
      key: "dns",
      tone: "bad",
      text: `Couldn't bind DNS port ${dnsUnbound} — .${tld} names won't resolve`,
      link: { label: "Change port", to: "/general" },
    });
  } else if (resolver === true) {
    healthy.push({ key: "resolver", tone: "ok", text: `.${tld} resolver installed` });
  }
  if (resolver !== true) {
    problems.push({
      key: "resolver",
      tone: "warn",
      text:
        resolver === false
          ? `.${tld} resolver isn't installed`
          : `.${tld} resolver status is unknown`,
      link: DOCTOR_FIX,
    });
  }

  if (report.web_unbound) {
    problems.push({
      key: "ports",
      tone: "bad",
      text: `Couldn't bind web ports ${report.web_unbound.http}/${report.web_unbound.https} — they're in use`,
      link: { label: "Change ports", to: "/general" },
    });
  } else if (!portsReady(report)) {
    const cause = report.foreign_web_listener === true ? "busy" : "not forwarded";
    problems.push({
      key: "ports",
      tone: "warn",
      text: `80/443 ${cause}, serving on :${report.http.bound} / :${report.https.bound}`,
      link: DOCTOR_FIX,
    });
  }

  return [...healthy, ...problems];
}

/** One group block of the Pinned view: its starred sites out of all its sites. */
export interface PinnedGroup {
  name: string;
  starred: SiteEntry[];
  total: number;
}

/** The Pinned view: group blocks with at least one star, then ungrouped stars. */
export interface PinnedView {
  groups: PinnedGroup[];
  ungrouped: SiteEntry[];
}

function byName(a: SiteEntry, b: SiteEntry): number {
  return a.name.localeCompare(b.name);
}

/**
 * The starred sites under their groups, in the groups' stored order. Membership
 * matches a known group ASCII-case-insensitively, like the Sites list; a
 * membership naming a missing group counts as ungrouped. Groups without a
 * starred site are left out.
 */
export function pinnedView(sites: readonly SiteEntry[], groups: GroupsState): PinnedView {
  const known = new Set(groups.order.map(asciiLower));
  const groupOf = (s: SiteEntry): string | null => {
    const g = memberGroup(groups, s.name);
    return g && known.has(asciiLower(g)) ? asciiLower(g) : null;
  };
  const sorted = [...sites].sort(byName);
  const blocks = groups.order.map((name) => {
    const members = sorted.filter((s) => groupOf(s) === asciiLower(name));
    return { name, starred: members.filter((s) => s.favourite === true), total: members.length };
  });
  return {
    groups: blocks.filter((g) => g.starred.length > 0),
    ungrouped: sorted.filter((s) => s.favourite === true && groupOf(s) === null),
  };
}

/** How many sites the Recently added view lists. */
export const RECENT_LIMIT = 7;

/** The most recently added sites, newest first (ties by name); undated sites are left out. */
export function recentlyAdded(sites: readonly SiteEntry[], limit = RECENT_LIMIT): SiteEntry[] {
  return sites
    .filter((s) => s.added_at !== undefined)
    .sort((a, b) => (b.added_at ?? 0) - (a.added_at ?? 0) || byName(a, b))
    .slice(0, limit);
}

/** The Services card's installed rows and how many of them are running. */
export function serviceSummary(services: readonly ServiceStatus[]): {
  rows: ServiceStatus[];
  running: number;
} {
  const rows = services.filter(isInstalled);
  return { rows, running: rows.filter((s) => s.state === "running").length };
}

/** The contributor tiles to show and how many others the "+N more" tile counts. */
export function contributorTiles(
  list: readonly Contributor[],
  total: number,
  shown: number,
): { shown: Contributor[]; more: number } {
  const tiles = list.slice(0, shown);
  return { shown: tiles, more: Math.max(0, total - tiles.length) };
}

/** The What's new summary line: a lead item and the faint "and N more" tail. */
export interface ReleaseSummary {
  lead: string;
  more: string | null;
}

/** The first feature and how many more, else the first fix, else the entry's summary. */
export function releaseSummary(entry: ReleaseNotesEntry): ReleaseSummary {
  const pick = (items: readonly ReleaseNoteItem[], one: string, many: string): ReleaseSummary => ({
    lead: items[0]?.title ?? "",
    more: items.length > 1 ? `and ${plural(items.length - 1, `more ${one}`, `more ${many}`)}` : null,
  });
  if (entry.features.length > 0) return pick(entry.features, "feature", "features");
  if (entry.fixes.length > 0) return pick(entry.fixes, "fix", "fixes");
  return { lead: entry.summary, more: null };
}

/** The expanded release's mono footer segments: items, features, fixes and distinct authors. */
export function releaseStats(entry: ReleaseNotesEntry): string[] {
  const all = [...entry.features, ...entry.fixes, ...entry.others];
  const authors = new Set(all.map((i) => i.author).filter((a): a is string => !!a));
  return [
    plural(all.length, "pull request", "pull requests"),
    plural(entry.features.length, "feature", "features"),
    plural(entry.fixes.length, "fix", "fixes"),
    plural(authors.size, "contributor", "contributors"),
  ];
}

/** How many features the expanded NEW column lists. */
export const FEATURES_SHOWN = 6;

/** How many fixes the expanded FIXED column lists. */
export const FIXES_SHOWN = 4;

/** The first `cap` items and how many are left over. */
export function cappedList<T>(items: readonly T[], cap: number): { shown: T[]; more: number } {
  return { shown: items.slice(0, cap), more: Math.max(0, items.length - cap) };
}

/** Every release the notes hold, the running one first. */
export function releaseList(notes: Pick<ReleaseNotesResponse, "current" | "earlier">): ReleaseNotesEntry[] {
  return notes.current ? [notes.current, ...notes.earlier] : [...notes.earlier];
}

/** The first-run checklist, given the report and the number of parked roots.
 *  Parking is done once a folder is parked or any site is linked. */
export function setupSteps(report: StatusReport, parkedRoots: number): SetupStep[] {
  return [
    { key: "trust", done: report.ca.trusted_system === true },
    { key: "php", done: report.php.length > 0 },
    {
      key: "park",
      done: parkedRoots > 0 || report.sites.parked > 0 || report.sites.linked > 0,
    },
  ];
}

/**
 * Whether the checklist still belongs on Overview: until PHP is installed and
 * a folder is parked (or a site linked). It stays through the steps between,
 * so installing PHP doesn't hide the park step. Trust alone doesn't keep it up:
 * the health well already flags an untrusted CA with its own Fix link.
 */
export function setupPending(steps: readonly SetupStep[]): boolean {
  return steps.some((s) => s.key !== "trust" && !s.done);
}

/** The first step still to do, or `null` when every step is done. */
export function nextStep(steps: readonly SetupStep[]): SetupStepKey | null {
  return steps.find((s) => !s.done)?.key ?? null;
}

/** An RFC 3339 timestamp as `15 Aug 2026` (UTC), or `""` when unparseable. */
export function formatReleaseDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString("en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
    timeZone: "UTC",
  });
}
