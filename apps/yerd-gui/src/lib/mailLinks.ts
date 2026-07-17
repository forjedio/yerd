/** Schemes a captured email may link to that we route to the OS browser (or the
 *  OS handler, for mailto/tel). Everything else - relative paths, in-page `#`
 *  anchors, `javascript:`, `data:`, `file:` - is ignored so a message can never
 *  drive navigation anywhere but a real external target the user chose. */
const OPENABLE_SCHEMES = new Set(["http:", "https:", "mailto:", "tel:"]);

// ── plain-text URL linkification ──────────────────────────────────────────

/**
 * URL pattern that matches http/https/mailto/tel links in plain text. Stops at
 * whitespace and common trailing punctuation (`.`, `,`, `)`, `]`, `>`, `"`,
 * `'`) that are unlikely to be part of the URL itself.
 */
const URL_PATTERN =
  /(?:https?:\/\/|mailto:|tel:)[^\s"'<>)\]]+(?<![.,)])/g;

/**
 * Convert URLs in a plain-text email body into clickable `<a>` tags. The
 * output is intended for use with `v-html` so the text is HTML-escaped first
 * to prevent any message content from injecting markup; only the synthesised
 * `<a>` tags are trusted HTML.
 *
 * Each anchor carries a `data-url` attribute containing the validated URL so
 * the click handler can read it via event delegation without re-parsing.
 */
export function linkifyText(text: string): string {
  // Escape HTML special characters so raw message content can't inject tags.
  const escaped = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
  // Replace URL matches with anchor tags. `resolveExternalHref` validates the
  // scheme so only safe, openable URLs become links.
  return escaped.replace(URL_PATTERN, (raw) => {
    const url = resolveExternalHref(raw);
    if (!url) return raw;
    return `<a href="${url}" class="text-brand underline cursor-pointer" data-url="${url}">${raw}</a>`;
  });
}

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
 * Coerce an event target to an Element. Clicks on link text yield a Text node
 * (no `closest`); climb to `parentElement` so we can still find the `<a>`.
 * Duck-typed for cross-realm iframe targets where `instanceof` is unreliable.
 */
export function eventTargetElement(target: EventTarget | null): Element | null {
  if (!target || typeof target !== "object") return null;
  const node = target as { nodeType?: number; parentElement?: Element | null; closest?: unknown };
  // Node.ELEMENT_NODE === 1, Node.TEXT_NODE === 3
  if (node.nodeType === 3) return node.parentElement ?? null;
  if (typeof node.closest === "function") return target as Element;
  return node.parentElement ?? null;
}

/**
 * Classify a click that landed on `target` inside an email body. Returns `null`
 * when the click isn't on a link (leave it alone); otherwise a
 * {@link FrameLinkAction}: openable schemes go to the OS browser, same-document
 * `#` anchors are left to scroll, and everything else (relative, same-origin,
 * `javascript:`) is blocked so the sandboxed frame can't navigate itself away
 * from the email. Walks up to the nearest `<a href>` so clicks on nested content
 * (e.g. `<a><img></a>` or link text) resolve.
 *
 * The target comes from the iframe's own realm, whose `Element` differs from
 * this module's, so `instanceof Element` would always be false. We duck-type on
 * `closest` instead (absent on `document`/`window` targets) to stay realm-safe.
 */
export function resolveFrameLink(target: EventTarget | null): FrameLinkAction | null {
  // Prefer data-yerd-url (stamped by prepareHtmlBody) so we still open even if
  // the visible href was rewritten or is a same-document fragment placeholder.
  const el = eventTargetElement(target);
  const anchor = el?.closest?.("a[href], a[data-yerd-url]") ?? null;
  if (!anchor) return null;
  const stamped = anchor.getAttribute("data-yerd-url");
  const href = anchor.getAttribute("href");
  const url = resolveExternalHref(stamped) ?? resolveExternalHref(href);
  if (url) return { kind: "open", url };
  return isInPageFragment(href) ? { kind: "scroll" } : { kind: "block" };
}

/**
 * Strip executable / navigable hazards from a captured HTML email before it is
 * rendered in the host (Shadow DOM). Keeps inline styles and images so the
 * message still looks like email; removes scripts, frames, and inline handlers.
 */
export function sanitizeMailHtml(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const strip = new Set(["script", "iframe", "object", "embed", "link", "meta", "base", "form"]);
  for (const el of [...doc.body.querySelectorAll("*")]) {
    if (strip.has(el.tagName.toLowerCase())) {
      el.remove();
      continue;
    }
    for (const attr of [...el.attributes]) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on") || name === "srcdoc") {
        el.removeAttribute(attr.name);
        continue;
      }
      if (
        (name === "href" || name === "src" || name === "xlink:href") &&
        /^\s*javascript:/i.test(attr.value)
      ) {
        el.removeAttribute(attr.name);
      }
    }
  }
  return doc.body.innerHTML;
}

/**
 * Stamp openable `<a href>` tags with `data-yerd-url` before rendering.
 * The host click listener reads that attribute to open links in the OS browser.
 */
export function prepareHtmlBody(html: string): string {
  const doc = new DOMParser().parseFromString(sanitizeMailHtml(html), "text/html");
  for (const a of doc.querySelectorAll("a[href]")) {
    const url = resolveExternalHref(a.getAttribute("href"));
    if (!url) continue;
    a.setAttribute("data-yerd-url", url);
  }
  return doc.body?.innerHTML ?? html;
}
