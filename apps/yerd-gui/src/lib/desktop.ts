/**
 * Make the webview behave like a native desktop window rather than a browser.
 *
 *  - Blocks zoom: Ctrl/Cmd + wheel, and Ctrl/Cmd + (+ / - / 0).
 *  - Suppresses the web context menu everywhere except editable fields (so
 *    inputs keep cut/copy/paste).
 *  - Cancels ghost-dragging of images/links.
 *  - Turns off native text assistance (autocorrect, autocapitalize,
 *    spellcheck) on every form field, current and future.
 *
 * Text-selection is handled in CSS (style.css) so it stays declarative.
 */

/** True when the event target is a text field, so shortcuts can defer to typing. */
export function isEditable(el: EventTarget | null): boolean {
  const node = el as HTMLElement | null;
  if (!node?.tagName) return false;
  const tag = node.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || node.isContentEditable;
}

const ZOOM_KEYS = new Set(["+", "-", "=", "0"]);

/** Strip WKWebView's text substitution off a single field. */
function harden(el: Element): void {
  el.setAttribute("autocorrect", "off");
  el.setAttribute("autocapitalize", "off");
  el.setAttribute("spellcheck", "false");
}

function hardenWithin(root: ParentNode): void {
  root.querySelectorAll("input, textarea").forEach(harden);
}

/**
 * Disable macOS autocorrect/autocapitalize/spellcheck on all form fields. The
 * attributes are set at element creation (before focus) via a MutationObserver,
 * since WKWebView may latch the correction state when a field is focused.
 */
function disableTextAssist(): void {
  hardenWithin(document);
  const observer = new MutationObserver((records) => {
    for (const rec of records) {
      rec.addedNodes.forEach((node) => {
        if (!(node instanceof Element)) return;
        if (node.matches("input, textarea")) harden(node);
        hardenWithin(node);
      });
    }
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
}

export function initDesktopChrome(): void {
  // Ctrl/Cmd + wheel zoom.
  globalThis.addEventListener(
    "wheel",
    (e) => {
      if (e.ctrlKey || e.metaKey) e.preventDefault();
    },
    { passive: false },
  );

  // Ctrl/Cmd + (+/-/0) zoom shortcuts (main row and numpad).
  globalThis.addEventListener(
    "keydown",
    (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const numpad =
        e.code === "NumpadAdd" ||
        e.code === "NumpadSubtract" ||
        e.code === "Numpad0";
      if (ZOOM_KEYS.has(e.key) || numpad) e.preventDefault();
    },
    { passive: false },
  );

  // Native-feeling: no web context menu except in editable fields.
  globalThis.addEventListener("contextmenu", (e) => {
    if (!isEditable(e.target)) e.preventDefault();
  });

  // No ghost-drag of images/links.
  globalThis.addEventListener("dragstart", (e) => {
    if (!isEditable(e.target)) e.preventDefault();
  });

  disableTextAssist();
}
