/**
 * Theme store: a reactive light/dark/system preference, persisted in GUI
 * settings (localStorage) and applied live to the `.dark` class on <html>.
 *
 * The shadcn token system (style.css) keys dark mode off `.dark`. We resolve the
 * effective mode from `pref`:
 *   - "system" → follow the OS (webview media query, reconciled with Tauri's
 *     `window.theme()` which is more reliable on Linux/webkit2gtk, + subscribe);
 *   - "light"/"dark" → force it, ignoring the OS.
 *
 * `pref` is a module-level singleton ref, so the General tab's selector and the
 * startup `initTheme()` share one source of truth.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ref } from "vue";

export type ThemePref = "system" | "light" | "dark";

const STORAGE_KEY = "yerd.theme";

/** The user's preference (reactive, persisted). */
const pref = ref<ThemePref>(loadPref());
/** Latest OS dark-mode signal — only consulted while `pref === "system"`. */
let osDark = false;

function loadPref(): ThemePref {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // localStorage unavailable (e.g. unit env) — fall through to default.
  }
  return "system";
}

function applyDark(dark: boolean): void {
  document.documentElement.classList.toggle("dark", dark);
}

function effectiveDark(): boolean {
  if (pref.value === "system") return osDark;
  return pref.value === "dark";
}

function reapply(): void {
  applyDark(effectiveDark());
}

/** Set + persist the preference and re-render the theme immediately. */
export function setTheme(p: ThemePref): void {
  pref.value = p;
  try {
    localStorage.setItem(STORAGE_KEY, p);
  } catch {
    // Best-effort persistence; the in-memory pref still applies this session.
  }
  reapply();
}

/** Reactive accessor for views (e.g. the General tab's theme selector). */
export function useTheme() {
  return { pref, setTheme };
}

/** Wire up OS-theme tracking and apply the stored preference. Call once at boot. */
export function initTheme(): void {
  // 1) Instant best-guess from the webview media query (no flash).
  const mq = globalThis.matchMedia("(prefers-color-scheme: dark)");
  osDark = mq.matches;
  reapply();
  mq.addEventListener("change", (e) => {
    osDark = e.matches;
    reapply();
  });

  // 2) Authoritative OS theme via Tauri; reconcile + subscribe. Only changes the
  //    UI while pref === "system" (effectiveDark gates it).
  void (async () => {
    try {
      const win = getCurrentWindow();
      const t = await win.theme(); // "light" | "dark" | null
      if (t) {
        osDark = t === "dark";
        reapply();
      }
      await win.onThemeChanged(({ payload }) => {
        osDark = payload === "dark";
        reapply();
      });
    } catch {
      // Not running inside Tauri (unit tests, plain Vite) — step 1 stands.
    }
  })();
}
