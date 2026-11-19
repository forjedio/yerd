import type {
  MajorLine,
  NodePreferenceRef,
  NodeRelease,
  NodeSiteEntry,
  NodeSource,
} from "@/ipc/types";

/**
 * Pure derivations for the Node screen: labels, release tags, used-by counts
 * and the sites that resolve away from the global default. No I/O; the inputs
 * are the daemon's `list_node`, `available_node` and `node_sites` replies.
 */

/** The major of an exact version, e.g. `22` for `22.12.0`. */
export function majorOf(version: string): string {
  return version.split(".")[0] ?? "";
}

/** The LTS chip text, with the codename when known. */
export function ltsLabel(codename: string | undefined): string {
  return codename ? `LTS ${codename}` : "LTS";
}

/** How many sites resolve to each exact version; unresolved sites are skipped. */
export function usedByCounts(entries: NodeSiteEntry[] | null): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const e of entries ?? []) {
    if (e.effective) counts[e.effective] = (counts[e.effective] ?? 0) + 1;
  }
  return counts;
}

/** The USED BY cell: `1 site`, `N sites`, or a dash for none or unknown. */
export function usedByLabel(count: number | undefined): string {
  if (!count) return "—";
  return count === 1 ? "1 site" : `${count} sites`;
}

/** Which release-index chips a release gets. */
export interface ReleaseTags {
  current: boolean;
  eol: boolean;
}

/**
 * Current / EOL chips from the release index. `current` is the newest major
 * line when it is supported and not LTS; `eol` is a line marked end-of-life.
 * Both stay false until the index is loaded or when the line is not listed.
 */
export function releaseTags(release: NodeRelease, majors: MajorLine[] | null): ReleaseTags {
  const major = Number(majorOf(release.version));
  const line = majors?.find((l) => l.major === major);
  if (!majors || !line) return { current: false, eol: false };
  const newest = Math.max(...majors.map((l) => l.major));
  return {
    current: line.eol === false && !line.codename && line.major === newest,
    eol: line.eol === true,
  };
}

/** Where a site's version came from, as the off-default list shows it. */
export function sourceLabel(source?: NodeSource): string {
  switch (source) {
    case "nvmrc":
      return ".nvmrc";
    case "node-version":
      return ".node-version";
    case "site":
      return "saved";
    case "default":
      return "default";
    default:
      return "";
  }
}

/** One row of the "Sites not on the default" card. */
export interface OffDefaultSite {
  site: string;
  domain: string;
  source: string;
  chip: string;
  /** The site's selector resolves to no installed release. */
  unresolved: boolean;
  /** The daemon's resolution error, shown as the chip's tooltip. */
  title?: string;
}

function unresolvedSource(e: NodeSiteEntry): string {
  if (e.project_file) return sourceLabel(e.project_file.kind);
  return e.preference ? "saved" : "";
}

/**
 * Sites whose Node differs from the global default, plus sites that resolve to
 * nothing, in the daemon's order. With no default installed every resolved
 * site counts as off the default.
 */
export function offDefaultSites(
  entries: NodeSiteEntry[],
  defaultVersion: string | undefined,
  tld: string,
): OffDefaultSite[] {
  const rows: OffDefaultSite[] = [];
  for (const e of entries) {
    const domain = `${e.site}.${tld}`;
    if (e.effective) {
      if (e.effective === defaultVersion) continue;
      rows.push({
        site: e.site,
        domain,
        source: sourceLabel(e.source),
        chip: `Node ${majorOf(e.effective)}`,
        unresolved: false,
      });
    } else {
      rows.push({
        site: e.site,
        domain,
        source: unresolvedSource(e),
        chip: e.missing ? `Node ${e.missing} missing` : "Unresolved",
        unresolved: true,
        title: e.error,
      });
    }
  }
  return rows;
}

/** A cleared saved preference: the site's domain when it is a site, else its root. */
export function clearedLabel(ref: NodePreferenceRef, tld: string): string {
  return ref.site ? `${ref.site}.${tld}` : ref.root;
}
