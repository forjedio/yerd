import { describe, expect, it } from "vitest";

import {
  buildMailFrameDocument,
  eventTargetElement,
  linkifyText,
  listRemoteContentUrls,
  prepareHtmlBody,
  resolveExternalHref,
  resolveFrameLink,
} from "./mailLinks";

describe("resolveExternalHref", () => {
  it("returns absolute http(s) URLs to open", () => {
    expect(resolveExternalHref("http://example.com")).toBe("http://example.com/");
    expect(resolveExternalHref("https://example.com/path?q=1#frag")).toBe(
      "https://example.com/path?q=1#frag",
    );
  });

  it("honours mailto and tel links", () => {
    expect(resolveExternalHref("mailto:hi@example.com")).toBe("mailto:hi@example.com");
    expect(resolveExternalHref("tel:+15551234567")).toBe("tel:+15551234567");
  });

  it("normalises scheme case and trims surrounding whitespace", () => {
    expect(resolveExternalHref("  HTTPS://Example.com  ")).toBe("https://example.com/");
  });

  it("ignores relative, protocol-relative, and in-page anchor links", () => {
    expect(resolveExternalHref("/dashboard")).toBeNull();
    expect(resolveExternalHref("../up")).toBeNull();
    expect(resolveExternalHref("//example.com")).toBeNull();
    expect(resolveExternalHref("#section")).toBeNull();
  });

  it("ignores unsupported and dangerous schemes", () => {
    expect(resolveExternalHref("javascript:alert(1)")).toBeNull();
    expect(resolveExternalHref("data:text/html,<h1>hi</h1>")).toBeNull();
    expect(resolveExternalHref("file:///etc/passwd")).toBeNull();
  });

  it("ignores empty, whitespace, and nullish input", () => {
    expect(resolveExternalHref("")).toBeNull();
    expect(resolveExternalHref("   ")).toBeNull();
    expect(resolveExternalHref(null)).toBeNull();
    expect(resolveExternalHref(undefined)).toBeNull();
  });
});

describe("resolveFrameLink", () => {
  function crossRealmDocument(): Document {
    const iframe = document.createElement("iframe");
    document.body.append(iframe);
    const doc = iframe.contentDocument;
    if (!doc) throw new Error("no iframe contentDocument");
    return doc;
  }

  function anchorWith(href: string, child?: string): Element {
    const doc = crossRealmDocument();
    const a = doc.createElement("a");
    a.setAttribute("href", href);
    if (child) a.innerHTML = child;
    doc.body.append(a);
    return a;
  }

  it("opens an external link when the click lands on the anchor itself", () => {
    expect(resolveFrameLink(anchorWith("https://example.com"))).toEqual({
      kind: "open",
      url: "https://example.com/",
    });
  });

  it("walks up from nested content (e.g. an image inside the link)", () => {
    const a = anchorWith("https://example.com", "<img alt='logo'>");
    expect(resolveFrameLink(a.querySelector("img"))).toEqual({
      kind: "open",
      url: "https://example.com/",
    });
  });

  it("walks up from a text node inside the link (the common click target)", () => {
    const a = anchorWith("https://msi-portal.test/materials");
    a.appendChild(a.ownerDocument.createTextNode("View all materials"));
    const text = a.firstChild;
    expect(text?.nodeType).toBe(Node.TEXT_NODE);
    expect(eventTargetElement(text)).toBe(a);
    expect(resolveFrameLink(text)).toEqual({
      kind: "open",
      url: "https://msi-portal.test/materials",
    });
  });

  it("lets same-document `#` anchors scroll", () => {
    expect(resolveFrameLink(anchorWith("#section"))).toEqual({ kind: "scroll" });
    expect(resolveFrameLink(anchorWith("#"))).toEqual({ kind: "scroll" });
  });

  it("blocks in-frame navigation for relative and same-origin links", () => {
    expect(resolveFrameLink(anchorWith("/dashboard"))).toEqual({ kind: "block" });
    expect(resolveFrameLink(anchorWith("../up"))).toEqual({ kind: "block" });
    expect(resolveFrameLink(anchorWith(""))).toEqual({ kind: "block" });
    expect(resolveFrameLink(anchorWith("javascript:alert(1)"))).toEqual({ kind: "block" });
  });

  it("returns null when the click isn't on a link", () => {
    const doc = crossRealmDocument();
    const p = doc.createElement("p");
    doc.body.append(p);
    expect(resolveFrameLink(p)).toBeNull();
    expect(resolveFrameLink(null)).toBeNull();
  });

  it("opens via data-url from legacy plain-text linkify", () => {
    const doc = crossRealmDocument();
    const a = doc.createElement("a");
    a.setAttribute("href", "#");
    a.setAttribute("data-url", "https://example.com/");
    doc.body.append(a);
    expect(resolveFrameLink(a)).toEqual({
      kind: "open",
      url: "https://example.com/",
    });
  });

  it("opens via data-yerd-url when present", () => {
    const doc = crossRealmDocument();
    const a = doc.createElement("a");
    a.setAttribute("href", "#");
    a.setAttribute("data-yerd-url", "https://msi-portal.test/materials/share/abc");
    doc.body.append(a);
    expect(resolveFrameLink(a)).toEqual({
      kind: "open",
      url: "https://msi-portal.test/materials/share/abc",
    });
  });
});

describe("buildMailFrameDocument", () => {
  it("preserves inline head styles and strips remote stylesheet links by default", () => {
    const { head, body } = buildMailFrameDocument(`<!doctype html><html><head>
<style>.title { color: red; }</style>
<link rel="stylesheet" href="https://cdn.example.com/mail.css">
<meta http-equiv="refresh" content="0;url=https://evil.example">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'">
</head><body><p class="title">Hi</p></body></html>`);
    expect(head).toContain(".title { color: red; }");
    expect(head).not.toContain("cdn.example.com/mail.css");
    expect(head).not.toContain("refresh");
    expect(head).not.toContain("Content-Security-Policy");
    expect(body).toContain('class="title"');
  });

  it("keeps remote stylesheet links when loadRemoteContent is true", () => {
    const { head } = buildMailFrameDocument(
      `<!doctype html><html><head>
<link rel="stylesheet" href="https://cdn.example.com/mail.css">
</head><body></body></html>`,
      { loadRemoteContent: true },
    );
    expect(head).toContain('href="https://cdn.example.com/mail.css"');
  });

  it("stamps openable anchors, neutralizes href, and strips image maps", () => {
    const { body } = buildMailFrameDocument(
      `<map name="m"><area href="https://evil.example" shape="rect" coords="0,0,1,1"></map>
       <a href="https://ok.example">Ok</a>`,
    );
    expect(body).not.toContain("<map");
    expect(body).not.toContain("<area");
    expect(body).toContain('data-yerd-url="https://ok.example/"');
    expect(body).toMatch(/href="#"/);
    expect(body).not.toContain('href="https://ok.example');
  });

  it("strips scripts, inline handlers, and javascript: URLs", () => {
    const { body } = buildMailFrameDocument(
      `<p onclick="alert(1)"><script>alert(1)</script><a href="javascript:alert(1)">x</a><a href="https://ok.example">Ok</a></p>`,
    );
    expect(body).not.toContain("<script");
    expect(body).not.toContain("onclick");
    expect(body).not.toContain("javascript:");
    expect(body).toContain('data-yerd-url="https://ok.example/"');
  });

  it("strips svg and math markup", () => {
    const { body } = buildMailFrameDocument(
      `<svg onload="alert(1)"><text>x</text></svg><math><mi>x</mi></math><p>Hi</p>`,
    );
    expect(body).not.toContain("<svg");
    expect(body).not.toContain("<math");
    expect(body).toContain("Hi");
  });

  it("strips remote images by default", () => {
    const { body } = buildMailFrameDocument(
      `<img src="https://cdn.example.com/logo.png" alt="logo">`,
    );
    expect(body).not.toContain("cdn.example.com/logo.png");
  });

  it("keeps remote images when loadRemoteContent is true", () => {
    const { body } = buildMailFrameDocument(
      `<img src="https://cdn.example.com/logo.png" alt="logo">`,
      { loadRemoteContent: true },
    );
    expect(body).toContain('src="https://cdn.example.com/logo.png"');
  });

  it("keeps cid and data images without opt-in", () => {
    const { body } = buildMailFrameDocument(
      `<img src="cid:logo@local" alt="cid"><img src="data:image/png;base64,abc" alt="data">`,
    );
    expect(body).toContain('src="cid:logo@local"');
    expect(body).toContain('src="data:image/png;base64,abc"');
  });

  it("neutralizes remote CSS url() without opt-in", () => {
    const { head, body } = buildMailFrameDocument(`<!doctype html><html><head>
<style>.bg { background: url(https://cdn.example.com/bg.png); }</style>
</head><body><div style="background-image: url('https://cdn.example.com/x.png')"></div></body></html>`);
    expect(head).not.toContain("cdn.example.com");
    expect(body).not.toContain("cdn.example.com");
  });

  it("strips string-form @import without opt-in", () => {
    const { head } = buildMailFrameDocument(`<!doctype html><html><head>
<style>@import "https://cdn.example.com/mail.css"; .ok { color: red; }</style>
</head><body></body></html>`);
    expect(head).not.toContain("cdn.example.com");
    expect(head).toContain(".ok { color: red; }");
  });

  it("keeps string-form @import when loadRemoteContent is true", () => {
    const { head } = buildMailFrameDocument(
      `<!doctype html><html><head>
<style>@import "https://cdn.example.com/mail.css";</style>
</head><body></body></html>`,
      { loadRemoteContent: true },
    );
    expect(head).toContain('@import "https://cdn.example.com/mail.css"');
  });

  it("drops non-stylesheet link tags", () => {
    const { head } = buildMailFrameDocument(`<!doctype html><html><head>
<link rel="prefetch" href="https://evil.example/x">
<link rel="stylesheet" href="https://cdn.example.com/ok.css">
</head><body></body></html>`);
    expect(head).not.toContain("prefetch");
    expect(head).not.toContain("ok.css");
  });
});

describe("listRemoteContentUrls", () => {
  it("lists remote stylesheets, images, and CSS urls without loading them", () => {
    const refs = listRemoteContentUrls(`<!doctype html><html><head>
<link rel="stylesheet" href="https://cdn.example.com/mail.css">
<style>@import "https://cdn.example.com/imported.css"; .bg { background: url(https://cdn.example.com/bg.png); }</style>
</head><body>
<img src="https://cdn.example.com/logo.png" alt="logo">
<img src="cid:inline@local" alt="inline">
<div style="background-image: url('//cdn.example.com/x.png')"></div>
</body></html>`);
    expect(refs).toEqual([
      { url: "https://cdn.example.com/mail.css", kind: "stylesheet" },
      { url: "https://cdn.example.com/logo.png", kind: "image" },
      { url: "https://cdn.example.com/x.png", kind: "css-url" },
      { url: "https://cdn.example.com/imported.css", kind: "stylesheet" },
      { url: "https://cdn.example.com/bg.png", kind: "css-url" },
    ]);
  });

  it("dedupes the same URL used in multiple places", () => {
    const refs = listRemoteContentUrls(
      `<img src="https://cdn.example.com/logo.png"><img src="https://cdn.example.com/logo.png">`,
    );
    expect(refs).toEqual([
      { url: "https://cdn.example.com/logo.png", kind: "image" },
    ]);
  });

  it("returns an empty list when there is no remote content", () => {
    expect(listRemoteContentUrls(`<p>Hi <img src="cid:x"></p>`)).toEqual([]);
  });
});

describe("prepareHtmlBody", () => {
  it("stamps openable anchors with data-yerd-url and neutralizes href", () => {
    const out = prepareHtmlBody(
      `<p><a href="https://msi-portal.test/download">Download</a></p>`,
    );
    expect(out).toContain('data-yerd-url="https://msi-portal.test/download"');
    expect(out).toMatch(/href="#"/);
    expect(out).not.toContain('href="https://msi-portal.test/download"');
  });

  it("leaves non-openable anchors alone", () => {
    const out = prepareHtmlBody(`<a href="#section">Jump</a>`);
    expect(out).not.toContain("data-yerd-url");
    expect(out).toContain('href="#section"');
  });

  it("strips scripts and inline handlers before stamping", () => {
    const out = prepareHtmlBody(
      `<p onclick="alert(1)"><script>alert(1)</script><a href="https://ok.example">Ok</a></p>`,
    );
    expect(out).not.toContain("<script");
    expect(out).not.toContain("onclick");
    expect(out).toContain('data-yerd-url="https://ok.example/"');
  });

  it("strips image maps so area hrefs cannot navigate", () => {
    const out = prepareHtmlBody(
      `<img usemap="#m" src="https://example.com/x.png"><map name="m"><area href="https://evil.example" shape="rect" coords="0,0,10,10"></map>`,
    );
    expect(out).not.toContain("<map");
    expect(out).not.toContain("<area");
    expect(out).not.toContain("example.com/x.png");
  });
});

describe("linkifyText", () => {
  it("wraps http/https URLs in anchor tags", () => {
    const out = linkifyText("Visit https://example.com for details.");
    expect(out).toContain("<a ");
    expect(out).toContain('data-yerd-url="https://example.com/"');
    expect(out).toContain("Visit");
    expect(out).toContain("for details.");
  });

  it("wraps mailto links in anchor tags", () => {
    const out = linkifyText("Email us at mailto:hi@example.com please.");
    expect(out).toContain('data-yerd-url="mailto:hi@example.com"');
  });

  it("HTML-escapes message content before linkifying", () => {
    const out = linkifyText("<script>alert(1)</script> https://safe.example.com");
    expect(out).toContain("&lt;script&gt;");
    expect(out).not.toContain("<script>");
    expect(out).toContain('data-yerd-url="https://safe.example.com/"');
  });

  it("does not linkify javascript: or data: URLs", () => {
    const out = linkifyText("Bad: javascript:alert(1) and data:text/html,hi");
    expect(out).not.toContain("<a ");
  });

  it("strips trailing punctuation from URLs", () => {
    const out = linkifyText("See https://example.com/path.");
    expect(out.includes('data-yerd-url="https://example.com/path."')).toBe(false);
    expect(out).toContain('data-yerd-url="https://example.com/path"');
  });

  it("returns plain escaped text when there are no URLs", () => {
    const out = linkifyText("Hello & goodbye.");
    expect(out).toBe("Hello &amp; goodbye.");
    expect(out).not.toContain("<a ");
  });

  it("handles an empty string", () => {
    expect(linkifyText("")).toBe("");
  });

  it("does not let quote entities in a URL inject attributes", () => {
    const out = linkifyText(
      'Click https://example.test/x&quot;onmouseover=&quot;alert(1) here',
    );
    const doc = new DOMParser().parseFromString(out, "text/html");
    expect(doc.querySelector("[onmouseover]")).toBeNull();
    expect(doc.body.querySelectorAll("a")).toHaveLength(1);
    const anchor = doc.body.querySelector("a");
    expect(anchor?.getAttributeNames().sort()).toEqual(["class", "data-yerd-url", "href"]);
    expect(anchor?.getAttribute("href")).toBe("#");
  });
});
