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
 * carries `data-url` with the validated URL for event-delegation click
 * handling.
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
      anchor.href = url;
      anchor.dataset.url = url;
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
 * `#` anchors are left to scroll, and everything else (relative, same-origin,
 * `javascript:`) is blocked so the preview can't navigate away from the email.
 * Walks up to the nearest `<a href>` / `a[data-yerd-url]` so clicks on nested
 * content (e.g. `<a><img></a>` or link text) resolve.
 *
 * Prefers `data-yerd-url` (stamped by {@link prepareHtmlBody}) over the visible
 * `href` so openable links still work when the displayed href is a fragment
 * placeholder or was rewritten. Duck-types on `closest` for cross-realm
 * targets where `instanceof Element` would fail.
 */
export function resolveFrameLink(target: EventTarget | null): FrameLinkAction | null {
  const el = eventTargetElement(target);
  const anchor =
    el?.closest?.("a[href], a[data-yerd-url], area[href]") ?? null;
  if (!anchor) return null;
  const stamped = anchor.getAttribute("data-yerd-url");
  const href = anchor.getAttribute("href");
  const url = resolveExternalHref(stamped) ?? resolveExternalHref(href);
  if (url) return { kind: "open", url };
  return isInPageFragment(href) ? { kind: "scroll" } : { kind: "block" };
}

/** Child-document CSP for the sandboxed mail iframe (paired with `buildMailFrameDocument`). */
export const MAIL_FRAME_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; img-src data: http: https:; style-src 'unsafe-inline' http: https:";

const STRIP_TAGS = new Set([
  "script",
  "iframe",
  "object",
  "embed",
  "base",
  "form",
  "map",
  "area",
]);

/**
 * Trusted bootstrap script injected into the iframe `head` (not from email).
 * Forwards openable link clicks to the host via `postMessage` so link routing
 * works on WKWebView where parent `contentDocument` listeners are unreliable.
 * Email scripts remain blocked by sanitization + CSP `default-src 'none'`.
 */
export const MAIL_FRAME_CLICK_BRIDGE = `<script>
document.addEventListener("click",function(e){
  var el=e.target;
  if(el&&el.nodeType===3)el=el.parentElement;
  while(el){
    var a=el.closest&&el.closest("a[href],a[data-yerd-url]");
    if(a){
      var stamped=a.getAttribute("data-yerd-url");
      var href=a.getAttribute("href")||"";
      var raw=(stamped||href).trim();
      if(/^(https?:|mailto:|tel:)/i.test(raw)){
        e.preventDefault();
        parent.postMessage({type:"yerd-mail-link",url:raw},"*");
        return;
      }
      if(href.trim().charAt(0)==="#")return;
      e.preventDefault();
      return;
    }
    el=el.parentElement;
  }
},true);
</script>`;

function sanitizeMailDocument(doc: Document): void {
  const roots: Element[] = [];
  if (doc.head) roots.push(doc.head);
  if (doc.body) roots.push(doc.body);
  for (const root of roots) {
    for (const el of [...root.querySelectorAll("*")]) {
      const tag = el.tagName.toLowerCase();
      if (tag === "link") {
        const rel = el.getAttribute("rel")?.toLowerCase() ?? "";
        if (!rel.includes("stylesheet")) el.remove();
        continue;
      }
      if (tag === "meta") {
        const equiv = el.getAttribute("http-equiv")?.toLowerCase() ?? "";
        if (equiv === "refresh" || equiv === "set-cookie") el.remove();
        continue;
      }
      if (STRIP_TAGS.has(tag)) {
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
  }
}

/**
 * Sanitize a captured HTML email and stamp openable anchors with `data-yerd-url`.
 * Preserves `<head>` styles / stylesheet links so marketing emails keep their CSS.
 */
export function buildMailFrameDocument(html: string): { head: string; body: string } {
  const doc = new DOMParser().parseFromString(html, "text/html");
  sanitizeMailDocument(doc);
  for (const a of doc.querySelectorAll("a[href]")) {
    const url = resolveExternalHref(a.getAttribute("href"));
    if (url) a.setAttribute("data-yerd-url", url);
  }
  return {
    head: doc.head?.innerHTML ?? "",
    body: doc.body?.innerHTML ?? "",
  };
}

/**
 * Strip executable / navigable hazards from captured HTML (body fragment).
 * Prefer {@link buildMailFrameDocument} for iframe rendering so `<head>` styles survive.
 */
export function sanitizeMailHtml(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  sanitizeMailDocument(doc);
  return doc.body?.innerHTML ?? html;
}

/**
 * Stamp openable `<a href>` tags with `data-yerd-url` (body HTML only).
 * @deprecated Prefer {@link buildMailFrameDocument} for the mail iframe.
 */
export function prepareHtmlBody(html: string): string {
  const doc = new DOMParser().parseFromString(sanitizeMailHtml(html), "text/html");
  for (const a of doc.querySelectorAll("a[href]")) {
    const url = resolveExternalHref(a.getAttribute("href"));
    if (url) a.setAttribute("data-yerd-url", url);
  }
  return doc.body?.innerHTML ?? html;
}
