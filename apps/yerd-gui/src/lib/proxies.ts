import type { ProxyEntry, ProxyRuleEntry, SiteEntry } from "@/ipc/types";
import { siteHost, type SiteLike } from "@/lib/siteUrl";

/**
 * Coerce user upstream input into what the daemon's parser accepts: a bare
 * port (`3000` or `:3000`) means that port on loopback, a missing scheme
 * defaults to `http://`, then a trailing slash is stripped so a pasted
 * `http://host:port/` isn't rejected as a path. Strips only when a host follows
 * the scheme, leaving a bare `http://` intact (rejected honestly as no host).
 */
export function normalizeUpstream(raw: string): string {
  let u = raw.trim();
  const port = /^:?(\d+)\/*$/.exec(u)?.[1];
  if (port !== undefined) u = `127.0.0.1:${port}`;
  if (u !== "" && !/^https?:\/\//i.test(u)) u = `http://${u}`;
  return u.replace(/^(https?:\/\/.+?)\/+$/i, "$1");
}

/** An upstream as rows and summaries show it: without a leading `http://` (an
 *  `https://` scheme stays visible, since it changes what is forwarded). */
export function upstreamLabel(target: string): string {
  return target.replace(/^http:\/\//i, "");
}

/** Whether a typed upstream already carries a scheme (hides the `http://` hint). */
export function hasScheme(raw: string): boolean {
  return /^https?:\/\//i.test(raw.trim());
}

/** A proxy name the daemon accepts. Dotted names are fine: a proxy's apex may
 *  itself be a subdomain, e.g. `api.account` serving `api.account.test`. */
export function isValidProxyName(raw: string): boolean {
  return /^[a-z0-9-]+(\.[a-z0-9-]+)*$/i.test(raw.trim());
}

/** The proxy name from the Domain field: trimmed, with one trailing `.{tld}`
 *  dropped (case-insensitive), so a typed `grafana.test` beside the drawn
 *  `.test` suffix doesn't become `grafana.test.test`. */
export function proxyNameFromInput(raw: string, tld: string): string {
  const name = raw.trim();
  const suffix = `.${tld}`.toLowerCase();
  return name.toLowerCase().endsWith(suffix) ? name.slice(0, -suffix.length) : name;
}

/** A path prefix the daemon accepts for a path rule (it must start with `/`). */
export function isValidPrefix(raw: string): boolean {
  return raw.trim().startsWith("/");
}

/** The Proxies filter chips. */
export type ProxyChipKey = "all" | "domains" | "paths";

/** One Proxies filter chip. */
export interface ProxyChip {
  key: ProxyChipKey;
  label: string;
  count: number;
}

/** All, Domains and Paths with their counts; all three always show, as drawn. */
export function proxyChips(
  proxies: readonly ProxyEntry[],
  rules: readonly ProxyRuleEntry[],
): ProxyChip[] {
  return [
    { key: "all", label: "All", count: proxies.length + rules.length },
    { key: "domains", label: "Domains", count: proxies.length },
    { key: "paths", label: "Paths", count: rules.length },
  ];
}

/** A row in the Proxies list: a whole-domain proxy or a path rule on a site. */
export type ProxyItem =
  | { kind: "proxy"; key: string; proxy: ProxyEntry }
  | { kind: "rule"; key: string; rule: ProxyRuleEntry };

/** The selection and busy key of a whole-domain proxy. */
export function proxyKey(name: string): string {
  return `proxy:${name}`;
}

/** The selection and busy key of a path rule. */
export function ruleKey(site: string, prefix: string): string {
  return `rule:${site}${prefix}`;
}

/** One section of the Proxies list. */
export interface ProxySection {
  kind: "proxy" | "rule";
  label: string;
  items: ProxyItem[];
}

/**
 * The list's sections for the active chip: "Domains" (proxies by name) then
 * "Paths on sites" (rules by site, then prefix). Empty sections are dropped.
 */
export function proxySections(
  proxies: readonly ProxyEntry[],
  rules: readonly ProxyRuleEntry[],
  chip: ProxyChipKey,
): ProxySection[] {
  const out: ProxySection[] = [];
  if (chip !== "paths" && proxies.length) {
    out.push({
      kind: "proxy",
      label: "Domains",
      items: [...proxies]
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((proxy) => ({ kind: "proxy", key: proxyKey(proxy.name), proxy })),
    });
  }
  if (chip !== "domains" && rules.length) {
    out.push({
      kind: "rule",
      label: "Paths on sites",
      items: [...rules]
        .sort((a, b) => a.site.localeCompare(b.site) || a.prefix.localeCompare(b.prefix))
        .map((rule) => ({ kind: "rule", key: ruleKey(rule.site, rule.prefix), rule })),
    });
  }
  return out;
}

/** A `SiteLike` for the shared URL helpers: a whole-domain proxy is served on the
 *  same ports as sites, so it obeys the same bound-port and unbound rules. The
 *  primary domain matters: once one is set, the apex may no longer route here. */
export function proxyAsSite(p: ProxyEntry): SiteLike {
  return { name: p.name, secure: p.secure, primary_domain: p.primary_domain };
}

/** The proxy's address host: its primary domain, else `{name}.{tld}`. */
export function proxyHost(p: ProxyEntry, tld: string): string {
  return siteHost(proxyAsSite(p), tld);
}

/** The proxy's effective domains other than its primary, exact names before
 *  wildcards (none for a default proxy, which the daemon sends without
 *  `domains`). */
export function extraDomains(p: ProxyEntry, tld: string): string[] {
  const primary = proxyHost(p, tld);
  const rest = (p.domains ?? []).filter((d) => d !== primary);
  return [...rest.filter((d) => !d.startsWith("*.")), ...rest.filter((d) => d.startsWith("*."))];
}

/** The site a path rule belongs to, or null when it isn't in the site list. */
export function ruleSite(rule: ProxyRuleEntry, sites: readonly SiteEntry[]): SiteEntry | null {
  return sites.find((s) => s.name === rule.site) ?? null;
}

/** What the New proxy dialog would add. */
export type AddSummaryInput =
  | { mode: "domain"; name: string; url: string; tld: string }
  | { mode: "path"; site: string; prefix: string; url: string; tld: string };

/**
 * The New proxy dialog's footer line, e.g. `grafana.test → localhost:3000` or
 * `codestash.test/live → 127.0.0.1:8081`. Empty parts show placeholders.
 */
export function addSummary(input: AddSummaryInput): { from: string; to: string } {
  const url = input.url.trim();
  const to = url ? upstreamLabel(normalizeUpstream(url)) : "upstream";
  if (input.mode === "domain") {
    const name = proxyNameFromInput(input.name, input.tld);
    return { from: `${name || "name"}.${input.tld}`, to };
  }
  const site = input.site ? `${input.site}.${input.tld}` : `site.${input.tld}`;
  return { from: `${site}${input.prefix.trim() || "/path"}`, to };
}
