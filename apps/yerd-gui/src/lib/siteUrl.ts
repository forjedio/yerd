import type { Site, StatusReport } from "@/ipc/types";

/** The minimal site shape the URL helpers need - satisfied by a full `Site` and
 *  by the create wizard's in-progress form (which has no real `Site` yet). */
export type SiteLike = Pick<Site, "name" | "secure">;

/**
 * True when `.test` resolution is unavailable, so sites must be reached via the
 * `http://localhost/~{domain}` fallback rather than their `.test` domain. This
 * covers both the OS resolver not being active (`resolver_installed` is only
 * "on" when strictly `true`, tri-state aware) *and* the daemon failing to bind
 * its DNS responder port (`dns_unbound` set) - in which case names won't resolve
 * through Yerd even when the resolver is installed.
 */
export function isUnbound(report: StatusReport | null | undefined): boolean {
  return report?.resolver_installed !== true || report?.dns_unbound != null;
}

interface UnboundOpts {
  httpBound: number | undefined;
  tld: string;
}

/**
 * The `http://localhost/~{name}.{tld}` URL used when the resolver is off.
 * Always plain http (there is no localhost cert), and the port is omitted when
 * it is the default 80.
 */
export function unboundUrlFor(name: string, opts: UnboundOpts): string {
  // Guard against a non-positive bound port: the daemon reports `http.bound = 0`
  // in degraded mode (couldn't bind web ports). `?? 8080` would NOT catch 0, so
  // use a truthiness check to avoid emitting a malformed `:0` URL.
  const port = opts.httpBound && opts.httpBound > 0 ? opts.httpBound : 8080;
  const portPart = port === 80 ? "" : `:${port}`;
  return `http://localhost${portPart}/~${name}.${opts.tld}`;
}

/**
 * Browser URL for a site's "Open" action. When the resolver is active this is
 * the site's `.test` domain (honouring scheme + bound port); when it is off,
 * the localhost `/~` fallback (forced http, `secure` ignored).
 */
export function siteUrl(s: SiteLike, report: StatusReport | null | undefined): string {
  const tld = report?.tld ?? "test";
  if (isUnbound(report)) {
    return unboundUrlFor(s.name, { httpBound: report?.http.bound, tld });
  }
  const scheme = s.secure ? "https" : "http";
  const bound = s.secure ? report?.https.bound : report?.http.bound;
  const dflt = s.secure ? 443 : 80;
  const redirected = report?.port_redirect === true;
  const port = !redirected && bound && bound !== dflt ? `:${bound}` : "";
  return `${scheme}://${s.name}.${tld}${port}`;
}

/**
 * The plain WP Admin URL for a WordPress site - the site's own URL plus
 * `/wp-admin/`. Not pre-authenticated: this opens the ordinary WordPress
 * login screen. Used as the fallback when one-click login isn't available
 * (unbound/resolver-off mode, or a failed token mint) - see
 * `wpAdminLoginUrl` for the pre-authenticated variant. `siteUrl` never
 * returns a trailing slash in either branch, so straight concatenation is
 * safe here.
 */
export function wpAdminUrl(s: SiteLike, report: StatusReport | null | undefined): string {
  return `${siteUrl(s, report)}/wp-admin/`;
}

/**
 * The one-click, pre-authenticated WP Admin URL: `wpAdminUrl` plus the
 * single-use login token as a query param. `yerd-proxy` recognizes this
 * param on `/wp-admin` requests, validates + consumes the token, and signs
 * the browser in as the site's admin before redirecting - see
 * `bin/yerdd/src/wordpress_login.rs`. Never use this in unbound/resolver-off
 * mode (the token can never validate there - see that module's docs); callers
 * should check `isUnbound(report)` first and fall back to `wpAdminUrl`.
 */
export function wpAdminLoginUrl(
  s: SiteLike,
  report: StatusReport | null | undefined,
  token: string,
): string {
  return `${wpAdminUrl(s, report)}?yerd_login_token=${encodeURIComponent(token)}`;
}

/**
 * Tooltip / aria text for an "Open" affordance. Appends the http-only caveat
 * when the resolver is off (the site is reached via the localhost `/~`
 * fallback). Shared so every Open affordance shows the same target + caveat.
 */
export function openTitle(s: SiteLike, report: StatusReport | null | undefined): string {
  const url = siteUrl(s, report);
  return isUnbound(report)
    ? `Open ${url} - served over http://localhost (forced-HTTPS sites may not load)`
    : `Open ${url}`;
}
