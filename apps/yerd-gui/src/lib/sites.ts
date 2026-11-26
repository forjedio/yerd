import type { GroupsState, SiteEntry, SiteFramework, StatusReport } from "@/ipc/types";
import { FRAMEWORK_LABELS, portsReady } from "@/lib/overview";

/**
 * Pure derivations behind the Sites screen: filter chips, list sections, the
 * overflow cut, the amber warning strips and the inspector's labels. Nothing
 * here fetches or mutates, so the components stay thin and the rules are
 * table-tested.
 */

/**
 * ASCII-only case-fold, matching the daemon's `eq_ignore_ascii_case` group
 * identity; full-Unicode `toLowerCase` disagrees on non-ASCII names.
 */
export function asciiLower(s: string): string {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

/**
 * The group a site's membership names, read as an own property so a site named
 * like an `Object.prototype` member (`constructor`, `toString`) is not handed an
 * inherited function.
 */
export function memberGroup(groups: GroupsState, name: string): string | undefined {
  return Object.prototype.hasOwnProperty.call(groups.members, name)
    ? groups.members[name]
    : undefined;
}

/**
 * A domain as the daemon stores it (`yerd-core` `Domain::parse`): ASCII
 * lowercased with one trailing dot stripped, so `API.blog.test.` and
 * `api.blog.test` are the same domain.
 */
export function normalizeDomain(fqdn: string): string {
  const lowered = asciiLower(fqdn.trim());
  return lowered.endsWith(".") ? lowered.slice(0, -1) : lowered;
}

/** Whether `name` matches an existing site's name, ASCII-case-insensitively. */
export function siteNameTaken(name: string, existing: readonly string[]): boolean {
  const wanted = asciiLower(name.trim());
  return wanted !== "" && existing.some((n) => asciiLower(n) === wanted);
}

/** The name the old list used for ungrouped sites; still reserved for groups. */
export const RESERVED_GROUP = "Unallocated";

/**
 * Whether `name` (trimmed) can name a group: non-empty, not the reserved name,
 * and not already taken, compared ASCII-case-insensitively like the daemon.
 * `except` is the group being renamed, which may keep its own name.
 */
export function groupNameAvailable(
  name: string,
  order: readonly string[],
  except?: string | null,
): boolean {
  const n = asciiLower(name.trim());
  if (!n || n === asciiLower(RESERVED_GROUP)) return false;
  const from = except ? asciiLower(except) : "";
  return !order.some((g) => {
    const gl = asciiLower(g);
    return gl === n && gl !== from;
  });
}

/** The parent directory of a path (slice before the last `/` or `\`). */
export function parentDir(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i <= 0 ? p : p.slice(0, i);
}

function trimTrailingSeparators(p: string): string {
  const trimmed = p.replace(/[/\\]+$/, "");
  return trimmed === "" ? p : trimmed;
}

/** The site whose document root is `path` (trailing separators ignored),
 *  preferring a linked site over a parked twin the way the daemon does. */
export function siteAtPath(sites: readonly SiteEntry[], path: string): SiteEntry | null {
  const want = trimTrailingSeparators(path);
  const matching = sites.filter((s) => trimTrailingSeparators(s.document_root) === want);
  return matching.find((s) => s.kind === "linked") ?? matching[0] ?? null;
}

/** A filter chip's identity. */
export type ChipKey = "all" | "https" | "linked" | "not-default" | `framework:${SiteFramework}`;

/** One filter chip with its label and the number of sites it matches. */
export interface SiteChip {
  key: ChipKey;
  label: string;
  count: number;
}

/** Whether `site` passes the chip `key`. */
export function matchesChip(site: SiteEntry, key: ChipKey, defaultPhp: string): boolean {
  if (key === "all") return true;
  if (key === "https") return site.secure;
  if (key === "linked") return site.kind === "linked";
  if (key === "not-default") return defaultPhp !== "" && site.php !== defaultPhp;
  return site.framework === key.slice("framework:".length);
}

/**
 * The chip row: All, HTTPS, Linked, one chip per recognised framework present
 * (most sites first), then "Not on {default}" when a default PHP is set. Every
 * chip but All is dropped when it would match nothing.
 */
export function siteChips(sites: readonly SiteEntry[], defaultPhp: string): SiteChip[] {
  const count = (key: ChipKey) => sites.filter((s) => matchesChip(s, key, defaultPhp)).length;
  const frameworks = new Map<SiteFramework, number>();
  for (const s of sites) {
    if (s.framework && FRAMEWORK_LABELS[s.framework]) {
      frameworks.set(s.framework, (frameworks.get(s.framework) ?? 0) + 1);
    }
  }
  const frameworkChips: SiteChip[] = [...frameworks.entries()]
    .map(([fw, n]) => ({
      key: `framework:${fw}` as ChipKey,
      label: FRAMEWORK_LABELS[fw] ?? fw,
      count: n,
    }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  const chips: SiteChip[] = [
    { key: "all", label: "All", count: sites.length },
    { key: "https", label: "HTTPS", count: count("https") },
    { key: "linked", label: "Linked", count: count("linked") },
    ...frameworkChips,
  ];
  if (defaultPhp) {
    chips.push({ key: "not-default", label: `Not on ${defaultPhp}`, count: count("not-default") });
  }
  return chips.filter((c) => c.key === "all" || c.count > 0);
}

/** What a list section groups by. */
export type SectionKind = "group" | "dir" | "linked" | "parked";

/** One collapsible section of the site list. */
export interface SiteSection {
  /** Collapse-state key: the bare group name, `dir:<path>`, `:linked` or `:parked`. */
  key: string;
  kind: SectionKind;
  label: string;
  /** Set for a user group (its canonical casing). */
  groupName?: string;
  /** Set for a parked folder section. */
  dir?: string;
  sites: SiteEntry[];
}

/** Inputs to `buildSections`; `sites` is already filtered by chip and query. */
export interface BuildSectionsInput {
  sites: readonly SiteEntry[];
  groups: GroupsState;
  parked: readonly string[];
  /** A query or a chip is active: empty sections are hidden. */
  filtering: boolean;
}

function byName(a: SiteEntry, b: SiteEntry): number {
  return a.name.localeCompare(b.name);
}

/**
 * The list's sections: user groups in their stored order, then one section per
 * parked folder holding its ungrouped sites, then ungrouped linked sites, then
 * any ungrouped parked site whose folder matches no parked root (defensive, e.g.
 * a symlinked root). A membership naming a group that no longer exists counts as
 * ungrouped. Empty groups and folders stay visible while nothing is filtered so
 * they remain manageable; Linked and the trailing section show only with sites.
 */
export function buildSections(input: BuildSectionsInput): SiteSection[] {
  const { groups, parked, filtering } = input;
  const sites = [...input.sites].sort(byName);
  const known = new Set(groups.order.map(asciiLower));
  const groupOf = (s: SiteEntry): string | null => {
    const g = memberGroup(groups, s.name);
    return g && known.has(asciiLower(g)) ? asciiLower(g) : null;
  };

  const groupSections: SiteSection[] = groups.order.map((name) => ({
    key: name,
    kind: "group",
    label: name,
    groupName: name,
    sites: sites.filter((s) => groupOf(s) === asciiLower(name)),
  }));

  const ungrouped = sites.filter((s) => groupOf(s) === null);
  const ungroupedParked = ungrouped.filter((s) => s.kind === "parked");
  const roots = new Set(parked);
  const dirSections: SiteSection[] = parked.map((dir) => ({
    key: `dir:${dir}`,
    kind: "dir",
    label: dir,
    dir,
    sites: ungroupedParked.filter((s) => parentDir(s.document_root) === dir),
  }));
  const linked: SiteSection = {
    key: ":linked",
    kind: "linked",
    label: "Linked",
    sites: ungrouped.filter((s) => s.kind === "linked"),
  };
  const other: SiteSection = {
    key: ":parked",
    kind: "parked",
    label: "Other parked sites",
    sites: ungroupedParked.filter((s) => !roots.has(parentDir(s.document_root))),
  };

  const keepEmpty = (sec: SiteSection) =>
    sec.sites.length > 0 || (!filtering && (sec.kind === "group" || sec.kind === "dir"));
  return [...groupSections, ...dirSections, linked, other].filter(keepEmpty);
}

/** How many rows a collapsed-by-length section shows before "+ N more". */
export const OVERFLOW_LIMIT = 10;

/** The rows a section renders and how many sit behind its "+ N more" line. */
export function visibleRows(
  section: SiteSection,
  expanded: boolean,
  filtering: boolean,
): { rows: SiteEntry[]; hidden: number } {
  if (filtering || expanded || section.sites.length <= OVERFLOW_LIMIT) {
    return { rows: section.sites, hidden: 0 };
  }
  return {
    rows: section.sites.slice(0, OVERFLOW_LIMIT),
    hidden: section.sites.length - OVERFLOW_LIMIT,
  };
}

/** One amber strip under the Sites toolbar. Both actions open Doctor. */
export interface SiteWarning {
  key: "ports" | "ca";
  text: string;
  /** Shorter copy for narrow windows. */
  compactText: string;
  action: "Run Doctor" | "Trust now";
}

/**
 * The strips to show, ports first. A web-port bind failure or a privileged-port
 * fallback without a redirect adds the ports strip; an untrusted CA adds the CA
 * strip. Unknown trust (`null`) adds nothing: Doctor covers that case.
 */
export function siteWarnings(report: StatusReport | null | undefined): SiteWarning[] {
  if (!report) return [];
  const out: SiteWarning[] = [];
  const http = report.http.bound;
  const https = report.https.bound;
  if (report.web_unbound) {
    const { http: h, https: s } = report.web_unbound;
    out.push({
      key: "ports",
      text: `Yerd couldn't bind web ports ${h}/${s}, they're in use.`,
      compactText: `Web ports ${h}/${s} are busy.`,
      action: "Run Doctor",
    });
  } else if (!portsReady(report)) {
    const foreign = report.foreign_web_listener === true;
    out.push({
      key: "ports",
      text: foreign
        ? `Ports 80 and 443 are in use by another process — Yerd fell back to :${http} and :${https}, so URLs need the port.`
        : `Ports 80 and 443 aren't forwarded — Yerd is serving on :${http} and :${https}, so URLs need the port.`,
      compactText: foreign
        ? `Ports 80/443 are busy — Yerd fell back to :${http} and :${https}.`
        : `Ports 80/443 aren't forwarded — Yerd is on :${http} and :${https}.`,
      action: "Run Doctor",
    });
  }
  if (report.ca.trusted_system === false) {
    const text = "Browsers will warn on HTTPS sites until you trust Yerd's local certificate authority.";
    out.push({ key: "ca", text, compactText: text, action: "Trust now" });
  }
  return out;
}

const UNRECOGNISED = "PHP site";

/** The framework's display name, falling back to the legacy flags, else "PHP site". */
export function frameworkLabel(site: SiteEntry): string {
  const label = site.framework ? FRAMEWORK_LABELS[site.framework] : undefined;
  if (label) return label;
  if (site.is_laravel) return "Laravel";
  if (site.is_wordpress) return "WordPress";
  return UNRECOGNISED;
}

/** Whether the site's framework is recognised (anything but "PHP site"). */
export function frameworkRecognised(site: SiteEntry): boolean {
  return frameworkLabel(site) !== UNRECOGNISED;
}

/** The Framework row's subtitle: how requests reach PHP, prefixed when detected. */
export function frameworkDetail(site: SiteEntry): string {
  const detected = frameworkRecognised(site);
  let how = "";
  if (site.uses_front_controller === true) {
    how = `front controller at /${site.web_subpath ?? ""}`;
  } else if (site.uses_front_controller === false) {
    how = "runs .php files directly";
  }
  if (detected) return how ? `Detected · ${how}` : "Detected";
  return how ? how.charAt(0).toUpperCase() + how.slice(1) : "";
}

/** Domains served beyond the primary (0 for a default site). */
export function aliasCount(site: Pick<SiteEntry, "domains">): number {
  return site.domains && site.domains.length > 1 ? site.domains.length - 1 : 0;
}

/** The HTTPS row's subtitle in the site and proxy inspectors. */
export function httpsDetail(secure: boolean, trusted: boolean | null | undefined): string {
  if (!secure) return "Served over plain HTTP";
  const state = trusted === true ? "trusted" : trusted === false ? "not trusted" : "trust unknown";
  return `Certificate issued by Yerd local CA · ${state}`;
}

/** The inspector's status pill. */
export interface ServingState {
  tone: "ok" | "warn" | "bad";
  label: string;
}

/** Not serving when the web ports are unbound; shadowed when another site claims the apex. */
export function servingState(
  site: SiteEntry,
  report: StatusReport | null | undefined,
): ServingState {
  if (report?.web_unbound) return { tone: "bad", label: "Not serving" };
  if (site.apex_shadowed_by) return { tone: "warn", label: `Served by ${site.apex_shadowed_by}` };
  return { tone: "ok", label: "Serving" };
}

/**
 * A path with the home directory shown as `~`. `home` may be `null` (not yet
 * resolved, or unavailable), in which case the path is returned as is.
 */
export function tildify(path: string, home: string | null | undefined): string {
  if (!home) return path;
  const root = home.replace(/[/\\]+$/, "");
  if (path === root) return "~";
  return path.startsWith(`${root}/`) ? `~${path.slice(root.length)}` : path;
}

/** The inverse of {@link tildify} for a typed path: a leading `~` or `~/`
 *  becomes `home`. Anything else (or no `home`) is returned as is. */
export function untildify(path: string, home: string | null | undefined): string {
  if (!home) return path;
  const root = home.replace(/[/\\]+$/, "");
  if (path === "~") return root;
  return path.startsWith("~/") ? `${root}${path.slice(1)}` : path;
}

/** The Source row's subtitle. `dir` is the matching parked root, if any. */
export function sourceDetail(
  site: SiteEntry,
  dir: string | null,
  home: string | null | undefined,
): string {
  if (site.kind === "linked") return `Linked from ${tildify(site.document_root, home)}`;
  return dir
    ? `Parked in ${tildify(dir, home)}, served automatically`
    : "Parked, served automatically";
}

/** The directory actually served: the project folder plus its web sub-path. */
export function servedPath(site: SiteEntry): string {
  return site.web_subpath ? `${site.document_root}/${site.web_subpath}` : site.document_root;
}

/** A URL split around its authority's port, so the port can be highlighted. */
export interface UrlParts {
  /** Scheme and host. */
  text: string;
  /** The port digits, or `null` when the URL has none. */
  port: string | null;
  /** Everything after the authority (e.g. `/~blog.test`). */
  path: string;
}

/** Split `url` into scheme+host, port and path. */
export function urlParts(url: string): UrlParts {
  const m = /^([a-z][a-z0-9+.-]*:\/\/[^/:]+)(?::(\d+))?(.*)$/i.exec(url);
  if (!m) return { text: url, port: null, path: "" };
  return { text: m[1] ?? url, port: m[2] ?? null, path: m[3] ?? "" };
}
