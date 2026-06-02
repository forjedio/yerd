/**
 * Make the webview behave like a native desktop window rather than a browser.
 *
 *  - Blocks zoom: Ctrl/Cmd + wheel, and Ctrl/Cmd + (+ / - / 0).
 *  - Suppresses the web context menu everywhere except editable fields (so
 *    inputs keep cut/copy/paste).
 *  - Cancels ghost-dragging of images/links.
 *
 * Text-selection is handled in CSS (style.css) so it stays declarative.
 */

function isEditable(el: EventTarget | null): boolean {
  const node = el as HTMLElement | null;
  if (!node?.tagName) return false;
  const tag = node.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || node.isContentEditable;
}

const ZOOM_KEYS = new Set(["+", "-", "=", "0"]);

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
}
