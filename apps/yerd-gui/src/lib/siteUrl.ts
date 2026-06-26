import type { Site, StatusReport } from "@/ipc/types";

/**
 * True when the OS `.test` resolver is not active, so sites must be reached via
 * the `http://localhost/~{domain}` fallback rather than their `.test` domain.
 * Tri-state aware: `resolver_installed` is only "on" when strictly `true`.
 */
export function isUnbound(report: StatusReport | null | undefined): boolean {
  return report?.resolver_installed !== true;
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
  const port = opts.httpBound ?? 8080;
  const portPart = port === 80 ? "" : `:${port}`;
  return `http://localhost${portPart}/~${name}.${opts.tld}`;
}

/**
 * Browser URL for a site's "Open" action. When the resolver is active this is
 * the site's `.test` domain (honouring scheme + bound port); when it is off,
 * the localhost `/~` fallback (forced http, `secure` ignored).
 */
export function siteUrl(s: Site, report: StatusReport | null | undefined): string {
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
