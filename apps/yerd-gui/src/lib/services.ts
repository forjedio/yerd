/**
 * Pure derivations for the Services screen. No I/O; inputs are the daemon's
 * `list_services`, `list_databases` and status replies.
 */
import type { ServiceRunState, ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";
import { plural } from "@/lib/utils";

/** A per-site instance (Reverb) has no installed version, but it is still a
 *  configured, startable instance identified by its linked site. */
export function isPerSite(s: ServiceStatus): boolean {
  return !!s.site;
}

/** Whether the row is a configured instance (installed engine, or a per-site
 *  instance) rather than an uninstalled single-instance engine. */
export function isInstalled(s: ServiceStatus): boolean {
  return s.installed_versions.length > 0 || isPerSite(s);
}

/** Whether Start applies: a configured instance that is not running. */
export function canStart(s: ServiceStatus): boolean {
  return isInstalled(s) && s.state !== "running";
}

/** Whether Stop applies: running, or failed with a process to clean up. */
export function canStop(s: ServiceStatus): boolean {
  return s.state === "running" || s.state === "failed";
}

/** The version to show: the active/selected one, falling back to what's on disk. */
export function versionLabel(s: ServiceStatus): string {
  return s.selected_version ?? s.installed_versions[s.installed_versions.length - 1] ?? "-";
}

/** Order two dotted service versions numerically (`9.10.0` after `9.9.1`); a
 *  missing or non-numeric segment counts as 0. */
export function compareServiceVersions(a: string, b: string): number {
  const pa = a.split(".").map((p) => Number.parseInt(p, 10) || 0);
  const pb = b.split(".").map((p) => Number.parseInt(p, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const diff = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** The engines whose stored data an older version can't open (MySQL and
 *  MariaDB keep one shared datadir; PostgreSQL clusters are per major and an
 *  older major can't read a newer one), so the GUI never switches them down. */
const NO_DOWNGRADE = new Set(["mysql", "mariadb", "postgres"]);

/** Whether switching this engine to an older version is refused. */
export function blocksDowngrade(typeId: string): boolean {
  return NO_DOWNGRADE.has(typeId);
}

/** The version a Change replaces: the selected one, else the newest installed. */
export function currentVersion(s: ServiceStatus): string | null {
  if (s.selected_version) return s.selected_version;
  const sorted = [...s.installed_versions].sort(compareServiceVersions);
  return sorted[sorted.length - 1] ?? null;
}

/** One option in the Change version picker. */
export interface VersionChoice {
  value: string;
  label: string;
  downgrade: boolean;
}

/**
 * The Change version picker: every available version not already installed,
 * oldest first, each flagged when it is older than `current`. The preselection
 * is the newest version newer than `current`, or "" when only older ones exist
 * so a downgrade is never one click away.
 */
export function versionChoices(
  available: readonly string[],
  installed: readonly string[],
  current: string | null,
): { options: VersionChoice[]; preselect: string } {
  const have = new Set(installed);
  const options = available
    .filter((v) => !have.has(v))
    .sort(compareServiceVersions)
    .map((v) => ({
      value: v,
      label: `v${v}`,
      downgrade: current !== null && compareServiceVersions(v, current) < 0,
    }));
  const newer = options.filter((o) => !o.downgrade);
  return { options, preselect: newer[newer.length - 1]?.value ?? "" };
}

function major(version: string): string {
  return version.split(".")[0] ?? version;
}

/** The note under the Change version picker, matching what the daemon does to
 *  the engine's data for this switch. */
export function changeVersionNote(
  typeId: string,
  name: string,
  current: string | null,
  target: string,
): { text: string; blocked: boolean } {
  if (!target) {
    return {
      text: blocksDowngrade(typeId)
        ? `Only older versions are offered. ${name} can't move to an older version, because data written by a newer version can't be read by an older one.`
        : "Only older versions are offered. Pick one to move to it.",
      blocked: true,
    };
  }
  const downgrade = current !== null && compareServiceVersions(target, current) < 0;
  if (downgrade && blocksDowngrade(typeId)) {
    return {
      text: `${name} can't move to an older version: data written by v${current} can't be read by v${target}. Pick a newer version instead.`,
      blocked: true,
    };
  }
  if (typeId === "meilisearch") {
    return {
      text: "Meilisearch indexes do not transfer between versions automatically. The previous version's data is retained; rebuild indexes with Laravel Scout.",
      blocked: false,
    };
  }
  if (typeId === "postgres" && current !== null && major(current) !== major(target)) {
    return {
      text: `Installs v${target}, restarts ${name} onto it, and removes v${current}. PostgreSQL ${major(target)} starts with an empty cluster: your PostgreSQL ${major(current)} databases stay on disk and come back if you switch back to a ${major(current)}.x version.`,
      blocked: false,
    };
  }
  return {
    text: "Installs the selected version, restarts the service onto it, and removes the current version. Your stored data is kept.",
    blocked: false,
  };
}

/** The version Uninstall removes, and the one its menu item and confirm name:
 *  the selected version when it is installed, else the newest installed. */
export function uninstallVersion(s: ServiceStatus): string {
  if (s.selected_version && s.installed_versions.includes(s.selected_version)) return s.selected_version;
  const sorted = [...s.installed_versions].sort(compareServiceVersions);
  return sorted[sorted.length - 1] ?? "";
}

/** Whether this instance carries a version to show (engines do; app servers like
 *  Reverb run against a site's PHP and have none). */
export function hasVersion(s: ServiceStatus): boolean {
  return !isPerSite(s) && (!!s.selected_version || s.installed_versions.length > 0);
}

/** Mirror of the daemon's `validate_db_name` for instant feedback when *creating*
 *  a database (the daemon re-validates authoritatively). Drop, backup, and restore
 *  act on names that already exist and are not gated by this. */
export function isValidDbName(name: string): boolean {
  return /^[A-Za-z_]\w{0,62}$/.test(name);
}

/** The message shown under an invalid new database name. */
export const DB_NAME_HINT =
  "Use letters, digits, and underscores; start with a letter or underscore (max 63).";

/** The list's three groups, in display order. */
export type ServiceGroup = "databases" | "cache" | "site";

/** One labelled group of installed services. */
export interface ServiceSection {
  group: ServiceGroup;
  label: string;
  items: ServiceStatus[];
}

const GROUP_LABELS: Record<ServiceGroup, string> = {
  databases: "Databases",
  cache: "Cache & search",
  site: "Per-site",
};

/** The group a service is listed under: per-site instances, SQL engines, then
 *  everything else (caches, search, any kind the GUI doesn't know yet). */
export function serviceGroup(s: ServiceStatus): ServiceGroup {
  if (isPerSite(s)) return "site";
  return s.supports_databases ? "databases" : "cache";
}

/** Installed services grouped for the list, in daemon order within a group and
 *  with empty groups left out. */
export function serviceSections(services: ServiceStatus[]): ServiceSection[] {
  const order: ServiceGroup[] = ["databases", "cache", "site"];
  return order
    .map((group) => ({
      group,
      label: GROUP_LABELS[group],
      items: services.filter((s) => isInstalled(s) && serviceGroup(s) === group),
    }))
    .filter((section) => section.items.length > 0);
}

/** The lowercase state word rows and the table show. */
export function statusWord(state: ServiceRunState): string {
  return state;
}

/** A list row's second line: `running · :3308 · 16 databases`. The count shows
 *  only for a running engine whose databases have been listed. */
export function statusLine(s: ServiceStatus, dbCount: number | null): string {
  const parts = [statusWord(s.state), `:${s.port}`];
  if (s.state === "running" && dbCount !== null) parts.push(plural(dbCount, "database", "databases"));
  return parts.join(" · ");
}

/** The inspector header's status: `Running · PID 56020`, `Stopped` or `Failed`. */
export function headerStatus(s: ServiceStatus): string {
  if (s.state === "running") return s.pid ? `Running · PID ${s.pid}` : "Running";
  return s.state === "failed" ? "Failed" : "Stopped";
}

/** The linked site's address: its primary domain when the site has one set,
 *  else `{site}.{tld}` (`reverb.test`). "" for a single-instance engine with no
 *  site. */
export function siteDomain(s: ServiceStatus, tld: string, sites: readonly SiteEntry[] = []): string {
  if (!s.site) return "";
  const linked = sites.find((site) => site.name === s.site);
  return linked?.primary_domain ?? `${s.site}.${tld}`;
}

/** The configured loopback address; the daemon binds every engine to 127.0.0.1. */
export function address(s: ServiceStatus): string {
  return `127.0.0.1:${s.port}`;
}

/** The compact table's DETAILS cell. */
export function detailsCell(s: ServiceStatus, dbCount: number | null): string {
  if (isPerSite(s)) return "WebSockets";
  if (s.supports_databases && s.state === "running" && dbCount !== null) {
    return plural(dbCount, "database", "databases");
  }
  return "—";
}

/** The database a Laravel parallel-testing copy was made from: Laravel names its
 *  copies `{database}_test_{n}`. Null for any other name. */
export function testCopyBase(name: string): string | null {
  const m = /^(.+)_test_\d+$/.exec(name);
  return m?.[1] ?? null;
}

/** One row of the Databases card, with the test copies folded into it. */
export interface DatabaseRow {
  name: string;
  copies: number;
}

/** The Databases card's counts for its footer. */
export interface DatabaseCounts {
  shown: number;
  total: number;
  hiddenCopies: number;
}

function copyRoot(name: string, present: Set<string>): string | null {
  let base = testCopyBase(name);
  if (base === null || !present.has(base)) return null;
  let root = base;
  for (;;) {
    base = testCopyBase(root);
    if (base === null || !present.has(base)) return root;
    root = base;
  }
}

/**
 * The Databases card's rows. A name counts as a test copy only when the
 * database it was copied from is also listed, so a real `report_test_2` with no
 * `report` stays a row. With `hideCopies` the copies are left out and counted
 * against the database they came from; otherwise every name is a row. The
 * filter (case-insensitive substring) applies after grouping.
 */
export function groupDatabases(
  names: string[],
  opts: { hideCopies: boolean; filter: string },
): { rows: DatabaseRow[]; counts: DatabaseCounts } {
  const present = new Set(names);
  const copies = new Map<string, number>();
  let hiddenCopies = 0;
  const kept: string[] = [];
  for (const name of names) {
    const root = opts.hideCopies ? copyRoot(name, present) : null;
    if (root === null) {
      kept.push(name);
      continue;
    }
    hiddenCopies += 1;
    copies.set(root, (copies.get(root) ?? 0) + 1);
  }
  const all = kept.map((name) => ({ name, copies: copies.get(name) ?? 0 }));
  const q = opts.filter.trim().toLowerCase();
  const rows = q ? all.filter((r) => r.name.toLowerCase().includes(q)) : all;
  return { rows, counts: { shown: rows.length, total: all.length, hiddenCopies } };
}

/** The Databases card's footer: `Showing 12 of 22 · 214 test copies hidden`. */
export function databasesFooter(counts: DatabaseCounts, hideCopies: boolean): string {
  const base = `Showing ${counts.shown} of ${counts.total}`;
  if (!hideCopies || counts.hiddenCopies === 0) return base;
  return `${base} · ${plural(counts.hiddenCopies, "test copy", "test copies")} hidden`;
}

/** The database the Connect card starts on: the first that isn't a test copy. */
export function defaultConnectDb(names: string[]): string {
  const present = new Set(names);
  return names.find((n) => copyRoot(n, present) === null) ?? names[0] ?? "";
}

/** One line of a `.env` block; `strong` lines are the ones a user edits or checks. */
export interface EnvLine {
  text: string;
  strong: boolean;
}

function lines(texts: string[], strong: string[] = []): EnvLine[] {
  return texts.map((text) => ({ text, strong: strong.some((k) => text.startsWith(`${k}=`)) }));
}

/** The `.env` block for a SQL engine. Credentials mirror the daemon's: SQL
 *  engines bind localhost with no password (root for MySQL/MariaDB, postgres for
 *  Postgres). A blank database becomes the `your_database` placeholder. */
export function sqlEnv(typeId: string, port: number, database: string): EnvLine[] {
  const connection = typeId === "postgres" ? "pgsql" : typeId;
  const user = typeId === "postgres" ? "postgres" : "root";
  return lines(
    [
      `DB_CONNECTION=${connection}`,
      "DB_HOST=127.0.0.1",
      `DB_PORT=${port}`,
      `DB_DATABASE=${database.trim() || "your_database"}`,
      `DB_USERNAME=${user}`,
      "DB_PASSWORD=",
    ],
    ["DB_DATABASE"],
  );
}

/** The cache, session and queue block for Redis (Valkey). */
export function redisEnv(port: number): EnvLine[] {
  return lines([
    "REDIS_CLIENT=phpredis",
    "REDIS_HOST=127.0.0.1",
    "REDIS_PASSWORD=null",
    `REDIS_PORT=${port}`,
    "",
    "CACHE_STORE=redis",
    "SESSION_DRIVER=redis",
    "QUEUE_CONNECTION=redis",
  ]);
}

/** The Laravel Scout block for Meilisearch; a blank prefix becomes `your_app`. */
export function meilisearchEnv(port: number, prefix: string): EnvLine[] {
  return lines([
    "SCOUT_DRIVER=meilisearch",
    `SCOUT_PREFIX=${prefix.trim() || "your_app"}_`,
    `MEILISEARCH_HOST=http://127.0.0.1:${port}`,
    "MEILISEARCH_KEY=",
  ]);
}

/** The Laravel S3 disk block for RustFS. Credentials mirror the daemon's
 *  fixed `rustfsadmin`/`rustfsadmin` key pair. */
export function rustfsEnv(port: number): EnvLine[] {
  return lines(
    [
      "FILESYSTEM_DISK=s3",
      "AWS_ACCESS_KEY_ID=rustfsadmin",
      "AWS_SECRET_ACCESS_KEY=rustfsadmin",
      "AWS_DEFAULT_REGION=us-east-1",
      "AWS_BUCKET=your_bucket",
      `AWS_ENDPOINT=http://127.0.0.1:${port}`,
      "AWS_USE_PATH_STYLE_ENDPOINT=true",
    ],
    ["AWS_ENDPOINT"],
  );
}

/** Where a browser reaches a site: host, scheme and port. */
export interface SitePublic {
  host: string;
  scheme: string;
  port: number;
}

/** The site's full public domain, scheme, and the port the browser actually
 *  uses, as `siteUrl` decides it: with an elevated-port redirect active
 *  (`port_redirect`) traffic reaches the default 443/80 even though the daemon
 *  bound 8443/8080; only a genuine rootless bind exposes the fallback port. */
export function reverbPublic(
  site: SiteEntry | null,
  siteName: string,
  report: StatusReport | null,
): SitePublic {
  const tld = report?.tld ?? "test";
  const host = site?.primary_domain ?? `${siteName}.${tld}`;
  const secure = site?.secure ?? false;
  const dflt = secure ? 443 : 80;
  const bound = secure ? report?.https.bound : report?.http.bound;
  const redirected = report?.port_redirect === true;
  const port = !redirected && bound && bound > 0 ? bound : dflt;
  return { host, scheme: secure ? "https" : "http", port };
}

/** A Reverb instance's two `.env` halves: the server (localhost) and the browser
 *  side (Echo), which reaches Reverb over the site's `/app` proxy. */
export interface ReverbEnv {
  server: EnvLine[];
  browser: EnvLine[];
  host: string;
}

/** The `.env` block for a Reverb instance. */
export function reverbEnv(port: number, pub: SitePublic): ReverbEnv {
  return {
    host: pub.host,
    server: lines(
      [
        "REVERB_APP_ID=my-app-id",
        "REVERB_APP_KEY=my-app-key",
        "REVERB_APP_SECRET=my-app-secret",
        "REVERB_HOST=127.0.0.1",
        `REVERB_PORT=${port}`,
        "REVERB_SCHEME=http",
      ],
      ["REVERB_PORT"],
    ),
    browser: lines(
      [
        'VITE_REVERB_APP_KEY="${REVERB_APP_KEY}"',
        `VITE_REVERB_HOST=${pub.host}`,
        `VITE_REVERB_PORT=${pub.port}`,
        `VITE_REVERB_SCHEME=${pub.scheme}`,
      ],
      ["VITE_REVERB_HOST"],
    ),
  };
}

/** The text Copy for .env puts on the clipboard. */
export function envText(env: EnvLine[] | ReverbEnv): string {
  if (Array.isArray(env)) return env.map((l) => l.text).join("\n");
  return [
    "# Reverb server - Yerd supervises this on localhost",
    ...env.server.map((l) => l.text),
    "",
    `# Laravel Echo (browser) - reaches Reverb over ${env.host} via Yerd's /app proxy`,
    ...env.browser.map((l) => l.text),
  ].join("\n");
}

/** Which `.env` block the Connect card shows, or `none` for a kind it has no
 *  snippet for. */
export type ConnectKind = "sql" | "redis" | "meilisearch" | "rustfs" | "site" | "none";

/** The Connect card's block for a service. */
export function connectKind(s: ServiceStatus): ConnectKind {
  if (isPerSite(s)) return "site";
  const type = s.type_id ?? s.service;
  if (type === "mysql" || type === "mariadb" || type === "postgres") return "sql";
  if (type === "redis") return "redis";
  if (type === "meilisearch") return "meilisearch";
  if (type === "rustfs") return "rustfs";
  return "none";
}

/** The last segment of a POSIX or Windows path. */
export function fileName(path: string): string {
  return path.split(/[/\\]/).pop() ?? path;
}

/** How a log line is painted: errors red, everything else plain. */
export function logLineTone(line: string): "error" | "plain" {
  return /\b(ERROR|FATAL|PANIC)\b|\[ERROR\]/.test(line) ? "error" : "plain";
}

/** The Uninstall confirm's body; `purge` is the "Also delete the stored data" choice. */
export function purgeCopy(name: string, dbCount: number | null, purge = false): string {
  if (purge) {
    if (dbCount === null) return `${name} stops, its binaries are removed and its stored data is deleted.`;
    return `${name} stops, its binaries are removed and ${plural(dbCount, "database", "databases")} ${dbCount === 1 ? "is" : "are"} deleted.`;
  }
  if (dbCount === null) {
    return `${name} stops and its binaries are removed. Your stored data stays on disk, so reinstalling picks it back up.`;
  }
  if (dbCount === 1) {
    return `${name} stops and its binaries are removed. Your 1 database stays on disk, so reinstalling picks it back up.`;
  }
  return `${name} stops and its binaries are removed. Your ${dbCount} databases stay on disk, so reinstalling picks them back up.`;
}
