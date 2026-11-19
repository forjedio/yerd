/**
 * Pure derivations for the Share screen. No I/O; inputs are the daemon's
 * `tunnel_status`, `list_named_tunnels` and `list_sites` replies.
 */
import type { CloudflaredStatus, Site, SiteHostname, TunnelInfo, TunnelRunState } from "@/ipc/types";
import { joinWithAnd, plural } from "@/lib/utils";

/** How a share is published: a temporary quick link or a hostname on the user's domain. */
export type ShareKind = "quick" | "domain";

/** A share's state. A domain share is `offline` while no named tunnel instance exists. */
export type ShareState = TunnelRunState | "offline";

/** One row of the Share list: a quick link or a site mapped onto the domain tunnel. */
export interface ShareItem {
  key: string;
  kind: ShareKind;
  site: string;
  /** The public URL, once known. */
  url: string | null;
  /** The URL without its scheme, as the rows show it. */
  host: string | null;
  state: ShareState;
  protected: boolean;
  lastError: string | null;
  warnings: string[];
}

/** One group of the Share list. */
export interface ShareSection {
  kind: ShareKind;
  label: string;
  items: ShareItem[];
}

/** A share's selection key. A site can hold a quick link and a domain share at once. */
export function shareKey(kind: ShareKind, site: string): string {
  return `${kind}:${site}`;
}

/** The single named tunnel instance the daemon supervises, in any state. */
export function namedEntry(tunnels: readonly TunnelInfo[]): TunnelInfo | null {
  return tunnels.find((t) => t.kind === "named") ?? null;
}

/** The named entry's warnings for one site. The daemon prefixes each with
 *  `"<site>: "`; the prefix is stripped because the share already names the site. */
export function namedWarningsFor(entry: TunnelInfo | null, site: string): string[] {
  const prefix = `${site}: `;
  return (entry?.warnings ?? []).filter((w) => w.startsWith(prefix)).map((w) => w.slice(prefix.length));
}

function hostOf(url: string): string {
  return url.replace(/^https?:\/\//, "").replace(/\/$/, "");
}

/** The Share list: Quick links in the daemon's order, then the sites on the
 *  domain tunnel by name. Empty groups are left out. */
export function shareSections(
  tunnels: readonly TunnelInfo[],
  namedSites: readonly SiteHostname[],
  zone: string | null,
): ShareSection[] {
  const entry = namedEntry(tunnels);
  const quick: ShareItem[] = tunnels
    .filter((t) => t.kind === "quick")
    .map((t) => ({
      key: shareKey("quick", t.site),
      kind: "quick",
      site: t.site,
      url: t.url ?? null,
      host: t.url ? hostOf(t.url) : null,
      state: t.state,
      protected: t.protected ?? false,
      lastError: t.last_error ?? null,
      warnings: t.warnings ?? [],
    }));
  const domain: ShareItem[] = [...namedSites]
    .sort((a, b) => a.site.localeCompare(b.site))
    .map((s) => ({
      key: shareKey("domain", s.site),
      kind: "domain",
      site: s.site,
      url: `https://${s.hostname}`,
      host: s.hostname,
      state: entry?.state ?? "offline",
      protected: s.protected ?? false,
      lastError: entry?.last_error ?? null,
      warnings: namedWarningsFor(entry, s.site),
    }));
  const sections: ShareSection[] = [
    { kind: "quick", label: "Quick links", items: quick },
    { kind: "domain", label: zone ? `Your domain · ${zone}` : "Your domain", items: domain },
  ];
  return sections.filter((s) => s.items.length > 0);
}

/** Whether the named tunnel can be started: sites are exposed and no instance
 *  exists, or the instance failed. A failed instance retries on its own, but the
 *  daemon only replaces it when its command changed, so a manual start is offered too. */
export function canStartNamed(entry: TunnelInfo | null, exposedCount: number): boolean {
  return exposedCount > 0 && (entry === null || entry.state === "failed");
}

const STATE_LABELS: Record<ShareState, string> = {
  running: "Running",
  connecting: "Connecting",
  reconnecting: "Reconnecting",
  failed: "Failed, retrying",
  offline: "Offline",
};

/** The status word for a share. Every reported failure is retrying: the daemon
 *  removes a quick share whose first start fails instead of reporting it. */
export function stateLabel(state: ShareState): string {
  return STATE_LABELS[state];
}

/** The status dot's colour for a share state. */
export function stateDotClass(state: ShareState): string {
  if (state === "running") return "bg-success";
  if (state === "failed") return "bg-destructive";
  if (state === "offline") return "bg-faint";
  return "bg-warning";
}

/** The suggested public hostname for a site: `{site}.{zone}` once the zone is
 *  known, otherwise a placeholder domain. */
export function suggestedHost(site: string, zone: string | null): string {
  return `${site}.${zone ?? "example.com"}`;
}

/** Where the active cloudflared came from. */
export function sourceLabel(status: CloudflaredStatus | null): string {
  return status?.source === "system" ? "on PATH" : "Yerd's copy";
}

/** `cloudflared 2026.6.1 · Yerd's copy`, the version left out when unknown. */
export function cloudflaredLine(status: CloudflaredStatus | null): string {
  const name = status?.version ? `cloudflared ${status.version}` : "cloudflared";
  return `${name} · ${sourceLabel(status)}`;
}

/** The TYPE fact for a share kind. */
export function typeFacts(kind: ShareKind): { title: string; detail: string } {
  return kind === "quick"
    ? { title: "Quick link", detail: "Temporary trycloudflare.com URL" }
    : { title: "Your domain", detail: "Stable hostname, survives restarts" };
}

/** The scheme a local site is published over. */
export function scheme(site: Site | undefined): "HTTPS" | "HTTP" {
  return site?.secure ? "HTTPS" : "HTTP";
}

/** The LOCAL SITE fact's second line. */
export function servedOver(site: Site | undefined): string {
  return `Served over ${scheme(site)}`;
}

/** The STATUS fact's second line: the share's error, else a hint for a down
 *  domain tunnel (pointing at Setup when no account is connected, since the
 *  tunnel can't start without one), else the cloudflared in use. */
export function statusDetail(
  item: ShareItem,
  cloudflared: CloudflaredStatus | null,
  loggedIn = true,
): string {
  if (item.lastError) return item.lastError;
  const down = item.state === "offline" || (item.kind === "domain" && item.state === "failed");
  if (down && !loggedIn) return "Connect your Cloudflare account in Setup to start it.";
  if (item.kind === "domain" && item.state === "failed") return "cloudflared exited. Restart to retry.";
  if (item.state === "offline") return "The tunnel isn't running. Start it to go live.";
  return cloudflaredLine(cloudflared);
}

/** The Setup tunnel row's chip: `Live · 2 sites` while running, else the state word. */
export function tunnelChip(entry: TunnelInfo | null, siteCount: number): string {
  if (!entry) return stateLabel("offline");
  if (entry.state === "running") return `Live · ${plural(siteCount, "site", "sites")}`;
  return stateLabel(entry.state);
}

/** The Cloudflare authorization URL from `cloudflared tunnel login` output, if printed yet. */
export function loginUrlFrom(lines: readonly string[]): string | null {
  for (const line of lines) {
    const m = line.match(/https:\/\/dash\.cloudflare\.com\/argotunnel\S*/);
    if (m) return m[0];
  }
  return null;
}

/** The Remove tunnel confirm's body, naming the sites it takes offline. */
export function removeTunnelCopy(sites: readonly string[]): string {
  if (sites.length === 0) {
    return "This deletes the tunnel from your Cloudflare account. DNS records created for it may need removing in the Cloudflare dashboard.";
  }
  return `This deletes the tunnel from your Cloudflare account and takes ${joinWithAnd(sites)} offline. DNS records created for them may need removing in the Cloudflare dashboard.`;
}
