/**
 * Pure derivations for the Share screen. No I/O; inputs are the daemon's
 * `tunnel_status`, `list_named_tunnels` and `list_sites` replies.
 */
import type {
  CloudflaredStatus,
  Site,
  SiteHostname,
  TunnelInfo,
  TunnelProvider,
  TunnelProviderStatus,
  TunnelRunState,
} from "@/ipc/types";
import { validateDomainShape } from "@/lib/domainValidation";
import { binaryLine, PROVIDER_ORDER, providerName, sourceLabel as providerSourceLabel } from "@/lib/providers";
import { joinWithAnd, plural } from "@/lib/utils";

/** How a share is published: a per-site share (a Cloudflare quick link, ngrok or
 *  zrok) or a hostname on the user's Cloudflare domain. */
export type ShareKind = "quick" | "domain";

/** A share's state. A domain share is `offline` while no named tunnel instance exists. */
export type ShareState = TunnelRunState | "offline";

/** One row of the Share list: a per-site share or a site mapped onto the domain tunnel. */
export interface ShareItem {
  key: string;
  provider: TunnelProvider;
  kind: ShareKind;
  site: string;
  /** Whether an ngrok share serves the configured static domain. */
  staticDomain: boolean;
  /** The public URL, once known. */
  url: string | null;
  /** The URL without its scheme, as the rows show it. */
  host: string | null;
  state: ShareState;
  protected: boolean;
  lastError: string | null;
  warnings: string[];
}

/** The Share list's provider filter. */
export type ShareFilter = "all" | TunnelProvider;

/** A share's selection key. A site can hold one share per provider plus a
 *  domain share at once. */
export function shareKey(provider: TunnelProvider, kind: ShareKind, site: string): string {
  return `${provider}:${kind}:${site}`;
}

/** The per-site share of `site` through `provider` (Cloudflare when `provider`
 *  is unknown), never the named-tunnel entry. */
export function siteShare(
  tunnels: readonly TunnelInfo[],
  site: string,
  provider: TunnelProvider | undefined,
): TunnelInfo | null {
  const want = provider ?? "cloudflare";
  return (
    tunnels.find((t) => t.site === site && t.kind === "quick" && (t.provider ?? "cloudflare") === want) ?? null
  );
}

/** What a client does when a per-site start reply lacks the share it asked for. */
export interface ProviderStartCheck {
  /** The daemon ignored the requested provider: report an error, not success. */
  unsupported: boolean;
  /** Stop the site's Cloudflare share, since this start created it. */
  stopCloudflare: boolean;
}

/**
 * Check a `start_quick_tunnel` reply against the provider that was asked for.
 * An older daemon ignores `provider` and starts a Cloudflare quick link, so a
 * reply with no share of `site` through a non-Cloudflare `provider` is
 * unsupported. The stray Cloudflare share is stopped only when `before` (the
 * tunnel list ahead of the start, or null when unknown) shows it didn't exist.
 */
export function checkProviderStart(
  site: string,
  provider: TunnelProvider | undefined,
  before: readonly TunnelInfo[] | null,
  after: readonly TunnelInfo[],
): ProviderStartCheck {
  if (!provider || provider === "cloudflare" || siteShare(after, site, provider)) {
    return { unsupported: false, stopCloudflare: false };
  }
  const created = before !== null && !siteShare(before, site, "cloudflare") && !!siteShare(after, site, "cloudflare");
  return { unsupported: true, stopCloudflare: created };
}

/** The error a client shows when the daemon can't share through `provider`. */
export function unsupportedProviderMessage(provider: TunnelProvider): string {
  return `The running Yerd daemon doesn't support ${providerName(provider)} sharing yet - restart Yerd and try again`;
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

/** The Share list: every per-site share (any provider) plus the sites on the
 *  domain tunnel, sorted by site, then provider order, then quick before domain. */
export function shareItems(tunnels: readonly TunnelInfo[], namedSites: readonly SiteHostname[]): ShareItem[] {
  const entry = namedEntry(tunnels);
  const perSite: ShareItem[] = tunnels
    .filter((t) => t.kind === "quick")
    .map((t) => {
      const provider = t.provider ?? "cloudflare";
      return {
        key: shareKey(provider, "quick", t.site),
        provider,
        kind: "quick",
        site: t.site,
        staticDomain: t.static_domain ?? false,
        url: t.url ?? null,
        host: t.url ? hostOf(t.url) : null,
        state: t.state,
        protected: t.protected ?? false,
        lastError: t.last_error ?? null,
        warnings: t.warnings ?? [],
      };
    });
  const domain: ShareItem[] = namedSites.map((s) => ({
    key: shareKey("cloudflare", "domain", s.site),
    provider: "cloudflare",
    kind: "domain",
    site: s.site,
    staticDomain: false,
    url: `https://${s.hostname}`,
    host: s.hostname,
    state: entry?.state ?? "offline",
    protected: s.protected ?? false,
    lastError: entry?.last_error ?? null,
    warnings: namedWarningsFor(entry, s.site),
  }));
  const kindRank = (k: ShareKind) => (k === "quick" ? 0 : 1);
  return [...perSite, ...domain].sort(
    (a, b) =>
      a.site.localeCompare(b.site) ||
      PROVIDER_ORDER.indexOf(a.provider) - PROVIDER_ORDER.indexOf(b.provider) ||
      kindRank(a.kind) - kindRank(b.kind),
  );
}

/** The shares the list shows under `filter`. */
export function filterShares(items: readonly ShareItem[], filter: ShareFilter): ShareItem[] {
  return filter === "all" ? [...items] : items.filter((i) => i.provider === filter);
}

/** How a share is published, in the design's words. */
export function kindLabel(item: ShareItem): string {
  if (item.provider === "ngrok") return item.staticDomain ? "Reserved domain" : "Random URL";
  if (item.provider === "zrok") return "Public share";
  return item.kind === "quick" ? "Quick link" : "Your domain";
}

/** The PROVIDER fact's second line. */
export function kindDetail(item: ShareItem): string {
  if (item.provider === "ngrok") return item.staticDomain ? "Your static ngrok domain" : "ngrok picks the address";
  if (item.provider === "zrok") return "New zrok URL each start";
  return item.kind === "quick" ? "Temporary trycloudflare.com URL" : "Stable hostname, survives restarts";
}

/** The other shares of `item`'s site. */
export function siblingShares(items: readonly ShareItem[], item: ShareItem): ShareItem[] {
  return items.filter((i) => i.site === item.site && i.key !== item.key);
}

/** The providers `site` has no share through, in provider order, with whether
 *  each is ready. Cloudflare counts as shared only by a quick link, since
 *  "Also share" offers a quick link. */
export function unsharedProviders(
  items: readonly ShareItem[],
  site: string,
  providers: readonly TunnelProviderStatus[],
): { provider: TunnelProvider; ready: boolean }[] {
  return PROVIDER_ORDER.filter(
    (p) => !items.some((i) => i.site === site && i.provider === p && i.kind === "quick"),
  ).map((provider) => ({
    provider,
    ready: providers.find((s) => s.provider === provider)?.readiness === "ready",
  }));
}

/** The site whose ngrok share holds the static `domain`, else null. */
export function ngrokDomainHolder(items: readonly ShareItem[], domain: string | null | undefined): string | null {
  if (!domain) return null;
  const url = `https://${domain.toLowerCase()}`;
  return items.find((i) => i.provider === "ngrok" && i.url === url)?.site ?? null;
}

/** The inspector's subtitle under the site name. */
export function subtitle(item: ShareItem, localHost: string, over: string, tunnelName: string | null): string {
  const sharing = `Sharing ${localHost} over ${over}`;
  if (item.provider !== "cloudflare") {
    return `${sharing} through ${providerName(item.provider)} · ${kindLabel(item).toLowerCase()}`;
  }
  if (item.kind === "quick") return `${sharing} · anyone with the link can reach it`;
  const exposed = `${localHost} over ${over}`;
  return tunnelName ? `Exposed through tunnel ${tunnelName} · ${exposed}` : `Exposed on your domain · ${exposed}`;
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
 *  known, otherwise "" so no made-up domain is ever submitted. */
export function suggestedHost(site: string, zone: string | null): string {
  return zone ? `${site}.${zone}` : "";
}

/** The hostname field's placeholder: an example on the zone, or on
 *  example.com while the zone is unknown. */
export function hostPlaceholder(zone: string | null): string {
  return `site.${zone ?? "example.com"}`;
}

/** Whether `host` can be shared on `zone`: a well-formed hostname (no
 *  wildcard) that is the zone or sits under it. False while the zone is
 *  unknown, since the DNS route can only be created on the account's zone. */
export function isShareHostname(host: string, zone: string | null): boolean {
  if (!zone) return false;
  const h = host.trim().toLowerCase().replace(/\.$/, "");
  const z = zone.toLowerCase();
  if (h.includes("*") || validateDomainShape(h) !== null) return false;
  return h === z || h.endsWith(`.${z}`);
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

/** The scheme a local site is published over. */
export function scheme(site: Site | undefined): "HTTPS" | "HTTP" {
  return site?.secure ? "HTTPS" : "HTTP";
}

/** The LOCAL SITE fact's second line. */
export function servedOver(site: Site | undefined): string {
  return `Served over ${scheme(site)}`;
}

/** The STATUS fact's second line: the share's error, else for ngrok or zrok
 *  the binary in use, else a hint for a down domain tunnel (pointing at Setup
 *  when no account is connected, since the tunnel can't start without one),
 *  else the cloudflared in use. */
export function statusDetail(
  item: ShareItem,
  cloudflared: CloudflaredStatus | null,
  loggedIn = true,
  providerStatus: TunnelProviderStatus | null = null,
): string {
  if (item.lastError) return item.lastError;
  if (item.provider !== "cloudflare") {
    const name = providerName(item.provider);
    return providerStatus ? `${binaryLine(providerStatus, name)} · ${providerSourceLabel(providerStatus)}` : name;
  }
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
