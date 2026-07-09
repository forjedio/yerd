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

/** What to do with a click on an `<a href>` inside the email frame: `open` it in
 *  the OS browser, let a same-document `#` anchor `scroll`, or `block` an
 *  in-frame navigation the preview must never perform (relative/same-origin
 *  links would otherwise load app content over the email). */
export type FrameLinkAction =
  | { kind: "open"; url: string }
  | { kind: "scroll" }
  | { kind: "block" };

/** True for an in-page fragment link (`#`, `#section`): activating it only
 *  scrolls the current document, so the handler must let it proceed. */
function isInPageFragment(rawHref: string | null): boolean {
  return (rawHref?.trim() ?? "").startsWith("#");
}

/**
 * Classify a click that landed on `target` inside an email body. Returns `null`
 * when the click isn't on a link (leave it alone); otherwise a
 * {@link FrameLinkAction}: openable schemes go to the OS browser, same-document
 * `#` anchors are left to scroll, and everything else (relative, same-origin,
 * `javascript:`) is blocked so the sandboxed frame can't navigate itself away
 * from the email. Walks up to the nearest `<a href>` so clicks on nested content
 * (e.g. `<a><img></a>`) resolve.
 *
 * The target comes from the iframe's own realm, whose `Element` differs from
 * this module's, so `instanceof Element` would always be false. We duck-type on
 * `closest` instead (absent on `document`/`window` targets) to stay realm-safe.
 */
export function resolveFrameLink(target: EventTarget | null): FrameLinkAction | null {
  const anchor = (target as Element | null)?.closest?.("a[href]");
  if (!anchor) return null;
  const href = anchor.getAttribute("href");
  const url = resolveExternalHref(href);
  if (url) return { kind: "open", url };
  return isInPageFragment(href) ? { kind: "scroll" } : { kind: "block" };
}
