import DOMPurify from "isomorphic-dompurify";
import type { Config } from "dompurify";

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
 * Convert URLs in a plain-text email body into clickable `<a>` tags for
 * `v-html`. Anchors are built with DOM APIs (`textContent` / `.href` /
 * `dataset`) so message content cannot inject attributes via entity decoding
 * (e.g. `&quot;` inside a matched URL). Non-URL text is appended as text
 * nodes and therefore HTML-escaped by the browser when serialised.
 *
 * Only URLs that pass {@link resolveExternalHref} become links. Each anchor
 * carries `data-yerd-url` with the validated URL; `href` is `#` so navigation
 * cannot bypass the click handler.
 */
export function linkifyText(text: string): string {
  const container = document.createElement("div");
  let offset = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    const raw = match[0];
    const index = match.index ?? 0;
    if (index > offset) {
      container.append(document.createTextNode(text.slice(offset, index)));
    }
    const url = resolveExternalHref(raw);
    if (url) {
      const anchor = document.createElement("a");
      anchor.href = "#";
      anchor.dataset.yerdUrl = url;
      anchor.className = "text-brand underline cursor-pointer";
      anchor.textContent = raw;
      container.append(anchor);
    } else {
      container.append(document.createTextNode(raw));
    }
    offset = index + raw.length;
  }
  if (offset < text.length) {
    container.append(document.createTextNode(text.slice(offset)));
  }
  return container.innerHTML;
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

/** What to do with a click on an `<a href>` inside the email body: `open` it in
 *  the OS browser, let a same-document `#` anchor `scroll`, or `block` an
 *  in-document navigation the preview must never perform. */
export type FrameLinkAction =
  | { kind: "open"; url: string }
  | { kind: "scroll" }
  | { kind: "block" };

/** True for an in-page fragment link (`#`, `#section`): activating it only
 *  scrolls the current document, so the handler must let it proceed. */
function isInPageFragment(rawHref: string | null): boolean {
  return (rawHref?.trim() ?? "").startsWith("#");
}

const NODE_TEXT = 3;

/**
 * Coerce an event target to an Element. Clicks on link text yield a Text node
 * (`NODE_TEXT`, no `closest`); climb to `parentElement` so we can still find
 * the `<a>`. Duck-typed for cross-realm targets where `instanceof` is
 * unreliable.
 */
export function eventTargetElement(target: EventTarget | null): Element | null {
  if (!target || typeof target !== "object") return null;
  const node = target as { nodeType?: number; parentElement?: Element | null; closest?: unknown };
  if (node.nodeType === NODE_TEXT) return node.parentElement ?? null;
  if (typeof node.closest === "function") return target as Element;
  return node.parentElement ?? null;
}

/**
 * Classify a click that landed on `target` inside an email body. Returns `null`
 * when the click isn't on a link (leave it alone); otherwise a
 * {@link FrameLinkAction}: openable schemes go to the OS browser, same-document
 * `#` anchors are left to scroll, and everything else is blocked.
 * Prefers `data-yerd-url` (stamped by {@link buildMailFrameDocument}) over the
 * visible `href`.
 */
export function resolveFrameLink(target: EventTarget | null): FrameLinkAction | null {
  const el = eventTargetElement(target);
  const anchor =
    el?.closest?.("a[href], a[data-yerd-url], a[data-url], area[href]") ?? null;
  if (!anchor) return null;
  const stamped =
    anchor.getAttribute("data-yerd-url") ?? anchor.getAttribute("data-url");
  const href = anchor.getAttribute("href");
  const url = resolveExternalHref(stamped) ?? resolveExternalHref(href);
  if (url) return { kind: "open", url };
  return isInPageFragment(href) ? { kind: "scroll" } : { kind: "block" };
}

/** DOMPurify config for captured HTML email (head + body). */
const MAIL_PURIFY_CONFIG: Config = {
  WHOLE_DOCUMENT: true,
  ADD_TAGS: ["link", "style", "meta", "head", "body", "html"],
  ADD_ATTR: ["target", "rel", "media", "type", "as", "crossorigin", "http-equiv", "content", "name", "charset"],
  FORBID_TAGS: [
    "script",
    "iframe",
    "object",
    "embed",
    "form",
    "input",
    "button",
    "textarea",
    "select",
    "option",
    "map",
    "area",
    "svg",
    "math",
    "base",
    "noscript",
    "template",
    "foreignObject",
    "video",
    "audio",
    "source",
    "track",
  ],
  // Allow data-* so we can stamp data-yerd-url after purify (and keep benign
  // email data attributes). Event handlers are still stripped by DOMPurify.
  ALLOW_DATA_ATTR: true,
};

/**
 * Drop non-stylesheet `<link>` nodes and dangerous `<meta http-equiv>` values
 * that DOMPurify may still allow when `meta` / `link` are on the allowlist.
 */
function filterHeadHazards(doc: Document): void {
  for (const link of [...doc.querySelectorAll("link")]) {
    const rel = link.getAttribute("rel")?.toLowerCase() ?? "";
    if (!rel.includes("stylesheet")) link.remove();
  }
  for (const meta of [...doc.querySelectorAll("meta")]) {
    const equiv = meta.getAttribute("http-equiv")?.toLowerCase() ?? "";
    if (
      equiv === "refresh" ||
      equiv === "set-cookie" ||
      equiv === "content-security-policy"
    ) {
      meta.remove();
    }
  }
}

/**
 * Stamp openable anchors with `data-yerd-url` and neutralize `href` to `#` so
 * default navigation cannot bypass the host click handler.
 */
function stampOpenableAnchors(root: ParentNode): void {
  for (const a of root.querySelectorAll("a[href]")) {
    const url = resolveExternalHref(a.getAttribute("href"));
    if (url) {
      a.setAttribute("data-yerd-url", url);
      a.setAttribute("href", "#");
    }
  }
}

/**
 * Sanitize a captured HTML email with DOMPurify and stamp openable anchors.
 * Preserves `<style>` and remote stylesheet `<link>` so marketing emails keep CSS.
 */
export function buildMailFrameDocument(html: string): { head: string; body: string } {
  const cleaned = DOMPurify.sanitize(html, MAIL_PURIFY_CONFIG);
  const doc = new DOMParser().parseFromString(cleaned, "text/html");
  filterHeadHazards(doc);
  stampOpenableAnchors(doc);
  return {
    head: doc.head?.innerHTML ?? "",
    body: doc.body?.innerHTML ?? "",
  };
}

/**
 * Strip executable / navigable hazards from captured HTML (body fragment).
 * Prefer {@link buildMailFrameDocument} when head styles must survive.
 */
export function sanitizeMailHtml(html: string): string {
  return buildMailFrameDocument(html).body;
}

/**
 * Stamp openable `<a href>` tags with `data-yerd-url` (body HTML only).
 * @deprecated Prefer {@link buildMailFrameDocument} for the mail viewer.
 */
export function prepareHtmlBody(html: string): string {
  return buildMailFrameDocument(`<body>${html}</body>`).body;
}
