import { describe, expect, it } from "vitest";

import { openableHrefForTarget, resolveExternalHref } from "./mailLinks";

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

describe("openableHrefForTarget", () => {
  // Build the anchor inside a real iframe's document, so `target` comes from a
  // different realm than this module - exactly the production shape (clicks
  // originate in the email frame). A same-realm `document.createElement` anchor
  // would not exercise the cross-realm path where `instanceof Element` fails.
  function frameDoc(): Document {
    const iframe = document.createElement("iframe");
    document.body.append(iframe);
    const doc = iframe.contentDocument;
    if (!doc) throw new Error("no iframe contentDocument");
    return doc;
  }

  function anchorWith(href: string, child?: string): Element {
    const doc = frameDoc();
    const a = doc.createElement("a");
    a.setAttribute("href", href);
    if (child) a.innerHTML = child;
    doc.body.append(a);
    return a;
  }

  it("resolves the URL when the click lands on the anchor itself", () => {
    const a = anchorWith("https://example.com");
    expect(openableHrefForTarget(a)).toBe("https://example.com/");
  });

  it("walks up from nested content (e.g. an image inside the link)", () => {
    const a = anchorWith("https://example.com", "<img alt='logo'>");
    const img = a.querySelector("img");
    expect(openableHrefForTarget(img)).toBe("https://example.com/");
  });

  it("returns null for a non-openable anchor so the click is left alone", () => {
    expect(openableHrefForTarget(anchorWith("#section"))).toBeNull();
    expect(openableHrefForTarget(anchorWith("javascript:alert(1)"))).toBeNull();
  });

  it("returns null when the click isn't on a link", () => {
    const doc = frameDoc();
    const p = doc.createElement("p");
    doc.body.append(p);
    expect(openableHrefForTarget(p)).toBeNull();
    expect(openableHrefForTarget(null)).toBeNull();
  });
});
