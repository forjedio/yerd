/** Schemes a captured email may link to that we route to the OS browser (or the
 *  OS handler, for mailto/tel). Everything else - relative paths, in-page `#`
 *  anchors, `javascript:`, `data:`, `file:` - is ignored so a message can never
 *  drive navigation anywhere but a real external target the user chose. */
const OPENABLE_SCHEMES = new Set(["http:", "https:", "mailto:", "tel:"]);

/**
 * Decide what a clicked link in an email body should open. Returns the absolute
 * URL to hand to the OS (via the opener plugin), or `null` when the link should
 * be ignored.
 *
 * Only absolute URLs in {@link OPENABLE_SCHEMES} are honoured. Relative hrefs,
 * bare `#` anchors, and malformed values all fail `new URL()` (we pass no base
 * on purpose, so nothing resolves against the app's own origin) and return
 * `null`; `javascript:`/`data:`/`file:` parse but are not openable schemes.
 */
export function resolveExternalHref(rawHref: string | null | undefined): string | null {
  if (!rawHref) return null;
  const href = rawHref.trim();
  if (!href) return null;
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  return OPENABLE_SCHEMES.has(url.protocol) ? url.href : null;
}

/**
 * The external URL to open for a click that landed on `target` inside an email
 * body, or `null` when the click isn't on an openable link. Walks up to the
 * nearest `<a href>` so clicks on nested content (e.g. `<a><img></a>`) resolve,
 * then classifies its href via {@link resolveExternalHref}. Returning `null`
 * lets the caller leave the click alone, so same-document `#` anchors still
 * scroll instead of becoming inert.
 *
 * The target comes from the iframe's own realm, whose `Element` differs from
 * this module's, so `instanceof Element` would always be false. We duck-type on
 * `closest` instead (absent on `document`/`window` targets) to stay realm-safe.
 */
export function openableHrefForTarget(target: EventTarget | null): string | null {
  const anchor = (target as Element | null)?.closest?.("a[href]");
  return anchor ? resolveExternalHref(anchor.getAttribute("href")) : null;
}
