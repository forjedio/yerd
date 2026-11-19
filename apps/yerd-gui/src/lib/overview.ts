import type { SiteEntry, SiteFramework, StatusReport } from "@/ipc/types";
import { humaniseUptime } from "@/lib/utils";

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

/** One row of the status card's health well. */
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

/** No PHP installed and nothing parked or linked: the first-run trigger. */
export function emptyEnvironment(report: StatusReport): boolean {
  return report.php.length === 0 && report.sites.parked === 0 && report.sites.linked === 0;
}

/** The status card's state: degraded when the web ports are unbound, else by site count. */
export function statusKind(report: StatusReport, siteCount: number): StatusKind {
  if (report.web_unbound) return "not-serving";
  return siteCount === 0 ? "ready" : "serving";
}

function sitesNoun(count: number): string {
  return count === 1 ? "site" : "sites";
}

/** The headline for a status kind and site count. */
export function siteHeadline(kind: StatusKind, siteCount: number): SiteHeadline {
  if (kind === "ready") return { text: "No sites yet", showTld: false };
  if (kind === "not-serving") {
    return { text: `0 of ${siteCount} ${sitesNoun(siteCount)}`, showTld: true };
  }
  return { text: `${siteCount} ${sitesNoun(siteCount)}`, showTld: true };
}

/** The mono meta line under the headline, as segments the template joins with ` · `. */
export function metaSegments(report: StatusReport): string[] {
  const out = [
    `yerdd ${report.daemon_version || "unknown"}`,
    `up ${humaniseUptime(report.uptime_secs)}`,
    `pid ${report.daemon_pid}`,
  ];
  if (report.services && report.services.length > 0) {
    const running = report.services.filter((s) => s.state === "running").length;
    out.push(`${running} ${running === 1 ? "service" : "services"} running`);
  }
  if (report.php.length === 0) out.push("no PHP installed");
  return out;
}

const DOCTOR_FIX = { label: "Fix", to: "/doctor" };

/**
 * The health well's rows. Healthy CA and resolver checks collapse into one `ok`
 * row; each failing or unknown check gets its own `warn` row linking to Doctor.
 * A web-port bind failure is a `bad` row linking to Settings; a privileged-port
 * fallback without a redirect is a `warn` row linking to Doctor. Healthy ports
 * add nothing.
 */
export function healthRows(report: StatusReport): HealthRow[] {
  const tld = report.tld;
  const healthy: string[] = [];
  const problems: HealthRow[] = [];

  const ca = report.ca.trusted_system;
  if (ca === true) healthy.push("Local CA trusted");
  else {
    problems.push({
      key: "trust",
      tone: "warn",
      text: ca === false ? "Local CA isn't trusted" : "Local CA trust is unknown",
      link: DOCTOR_FIX,
    });
  }

  const resolver = report.resolver_installed;
  if (resolver === true) healthy.push(`.${tld} resolver installed`);
  else {
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
    const cause = report.foreign_web_listener === true ? "are in use" : "aren't forwarded";
    problems.push({
      key: "ports",
      tone: "warn",
      text: `Ports 80/443 ${cause} — serving on :${report.http.bound} / :${report.https.bound}`,
      link: DOCTOR_FIX,
    });
  }

  const rows: HealthRow[] = [];
  if (healthy.length > 0) rows.push({ key: "ok", tone: "ok", text: healthy.join(" · ") });
  return [...rows, ...problems];
}

/** The starred sites, sorted by name. */
export function favouriteSites(sites: readonly SiteEntry[]): SiteEntry[] {
  return sites.filter((s) => s.favourite === true).sort((a, b) => a.name.localeCompare(b.name));
}

/** A favourite row's second line: path, then `linked` and the framework when known. */
export function siteSubtitle(site: SiteEntry): string {
  const parts = [site.document_root];
  if (site.kind === "linked") parts.push("linked");
  const framework = site.framework ? FRAMEWORK_LABELS[site.framework] : undefined;
  if (framework) parts.push(framework);
  return parts.join(" · ");
}

/** The first-run checklist, given the report and the number of parked roots. */
export function setupSteps(report: StatusReport, parkedRoots: number): SetupStep[] {
  return [
    { key: "trust", done: report.ca.trusted_system === true },
    { key: "php", done: report.php.length > 0 },
    { key: "park", done: parkedRoots > 0 || report.sites.parked > 0 },
  ];
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
