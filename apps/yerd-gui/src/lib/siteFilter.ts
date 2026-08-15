import type { SiteEntry } from "@/ipc/types";

/** Every domain a site answers on: the default `<name>.<tld>` apex plus any
 *  extra domains, subdomains or wildcards configured for it. `domains` is
 *  omitted for an uncustomised site and is authoritative when present, but the
 *  apex is kept regardless so a site is always findable by its own name.
 */
function siteDomains(site: SiteEntry, tld: string): string[] {
  const domains = new Set([`${site.name}.${tld}`]);
  if (site.primary_domain) domains.add(site.primary_domain);
  for (const domain of site.domains ?? []) domains.add(domain);
  return [...domains];
}

/**
 * Whether `query` matches any domain the site serves, case-insensitively and as
 * a substring - so "admin." finds `codestash.test` when its only match is an
 * added domain like `admin.codestash.test`. An empty query matches everything.
 */
export function matchesSiteFilter(site: SiteEntry, tld: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  return siteDomains(site, tld).some((domain) => domain.toLowerCase().includes(q));
}
