/**
 * Sync the `.dark` class on <html> with the OS colour-scheme preference.
 *
 * The shadcn token system keys dark mode off a `.dark` class (see style.css).
 * We apply it in two steps:
 *   1. immediately from the webview's `prefers-color-scheme` (no flash), then
 *   2. reconcile with Tauri's `window.theme()` — which reads the real OS theme
 *      (more reliable than the webview media query on Linux/webkit2gtk) — and
 *      subscribe to OS theme changes.
 *
 * No manual override yet; the app just follows the system, light or dark.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";

function applyDark(dark: boolean): void {
  document.documentElement.classList.toggle("dark", dark);
}

export function initTheme(): void {
  // 1) Instant best-guess from the webview media query.
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  applyDark(mq.matches);
  mq.addEventListener("change", (e) => applyDark(e.matches));

  // 2) Authoritative: Tauri reads the actual OS theme; reconcile + subscribe.
  void (async () => {
    try {
      const win = getCurrentWindow();
      const theme = await win.theme(); // "light" | "dark" | null
      if (theme) applyDark(theme === "dark");
      await win.onThemeChanged(({ payload }) => applyDark(payload === "dark"));
    } catch {
      // Not running inside Tauri (unit tests, plain Vite) — step 1 stands.
    }
  })();
}
