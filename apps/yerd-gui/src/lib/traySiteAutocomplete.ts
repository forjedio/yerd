/**
 * Pure matching / ranking for the tray panel "Jump to site…" autocomplete.
 */
import type { SiteEntry } from "@/ipc/types";

export type TraySiteGroup = "Favorites" | "Recent" | "Sites";

export interface TraySiteSuggestion {
  site: SiteEntry;
  group: TraySiteGroup;
  /** Primary display label (domain or name). */
  label: string;
  /** Sublabel (project path). */
  sublabel: string;
}

export interface TrayAutocompletePrefs {
  favorites: readonly string[];
  recent: readonly string[];
  tld: string;
  /** Max suggestions returned (default 50). */
  limit?: number;
  /** Cap for empty-query favorites+recent combined (default 8). */
  emptyCap?: number;
}

/** Build grouped, ranked suggestions for the tray site autocomplete. */
export function buildTraySiteSuggestions(
  sites: readonly SiteEntry[],
  query: string,
  prefs: TrayAutocompletePrefs,
): TraySiteSuggestion[] {
  const limit = prefs.limit ?? 50;
  const emptyCap = prefs.emptyCap ?? 8;
  const favSet = new Set(prefs.favorites);
  const recentOrder = prefs.recent;
  const byName = new Map(sites.map((s) => [s.name, s]));

  const q = query.trim().toLowerCase();
  const matched = q
    ? sites.filter((s) => siteMatches(s, q, prefs.tld))
    : [...sites];

  if (!q) {
    const out: TraySiteSuggestion[] = [];
    const seen = new Set<string>();

    for (const name of prefs.favorites) {
      const site = byName.get(name);
      if (!site || seen.has(name)) continue;
      seen.add(name);
      out.push(toSuggestion(site, "Favorites", prefs.tld));
      if (out.length >= emptyCap) return out.slice(0, limit);
    }
    for (const name of recentOrder) {
      const site = byName.get(name);
      if (!site || seen.has(name)) continue;
      seen.add(name);
      out.push(toSuggestion(site, "Recent", prefs.tld));
      if (out.length >= emptyCap) return out.slice(0, limit);
    }
    const rest = matched
      .filter((s) => !seen.has(s.name))
      .sort((a, b) => a.name.localeCompare(b.name))
      .slice(0, Math.max(0, limit - out.length))
      .map((s) => toSuggestion(s, "Sites", prefs.tld));
    return [...out, ...rest].slice(0, limit);
  }

  // Rank: favorites first, then recent (MRU), then alpha.
  const recentIdx = new Map(recentOrder.map((n, i) => [n, i]));
  matched.sort((a, b) => {
    const af = favSet.has(a.name) ? 0 : 1;
    const bf = favSet.has(b.name) ? 0 : 1;
    if (af !== bf) return af - bf;
    const ar = recentIdx.has(a.name) ? (recentIdx.get(a.name) as number) : 9999;
    const br = recentIdx.has(b.name) ? (recentIdx.get(b.name) as number) : 9999;
    if (ar !== br) return ar - br;
    return a.name.localeCompare(b.name);
  });

  return matched.slice(0, limit).map((s) => {
    const group: TraySiteGroup = favSet.has(s.name)
      ? "Favorites"
      : recentIdx.has(s.name)
        ? "Recent"
        : "Sites";
    return toSuggestion(s, group, prefs.tld);
  });
}

export function siteMatches(site: SiteEntry, queryLower: string, tld: string): boolean {
  if (site.name.toLowerCase().includes(queryLower)) return true;
  const domain = displayHost(site, tld).toLowerCase();
  if (domain.includes(queryLower)) return true;
  for (const d of site.domains ?? []) {
    if (d.toLowerCase().includes(queryLower)) return true;
  }
  const path = site.document_root.toLowerCase();
  if (path.includes(queryLower)) return true;
  // Path segment match (last few segments).
  for (const seg of site.document_root.split(/[/\\]/).filter(Boolean)) {
    if (seg.toLowerCase().includes(queryLower)) return true;
  }
  return false;
}

export function displayHost(site: SiteEntry, tld: string): string {
  return site.primary_domain ?? `${site.name}.${tld}`;
}

function toSuggestion(
  site: SiteEntry,
  group: TraySiteGroup,
  tld: string,
): TraySiteSuggestion {
  return {
    site,
    group,
    label: displayHost(site, tld),
    sublabel: site.document_root,
  };
}

/** Push `name` to the front of an MRU list, capped. */
export function pushRecent(recent: readonly string[], name: string, cap = 8): string[] {
  const next = [name, ...recent.filter((n) => n !== name)];
  return next.slice(0, cap);
}

/** Toggle a site name in the favorites list. */
export function toggleFavorite(favorites: readonly string[], name: string): string[] {
  return favorites.includes(name)
    ? favorites.filter((n) => n !== name)
    : [...favorites, name];
}
