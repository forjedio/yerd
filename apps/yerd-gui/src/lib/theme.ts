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
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ref } from "vue";

export type ThemePref = "system" | "light" | "dark";

const STORAGE_KEY = "yerd.theme";
/** Cross-window broadcast: each webview (main, mails, dumps) is a separate JS
 *  context + DOM, so a theme change in one is published to the others. */
const THEME_EVENT = "yerd:theme-changed";

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

/** Apply a preference locally: update the reactive ref, persist, and re-render. */
function applyPref(p: ThemePref): void {
  pref.value = p;
  try {
    localStorage.setItem(STORAGE_KEY, p);
  } catch {
    // Best-effort persistence; the in-memory pref still applies this session.
  }
  reapply();
}

/** Set + persist the preference, re-render immediately, and broadcast the change
 *  to the app's other windows (mails/dumps) so they switch in lockstep. */
export function setTheme(p: ThemePref): void {
  applyPref(p);
  // Other webviews don't see this window's localStorage write, so tell them.
  void emit(THEME_EVENT, p).catch(() => {
    // Not in Tauri (unit/dev) — nothing to broadcast.
  });
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

  // 3) Cross-window sync: when any window changes the theme, apply it here too,
  //    so the popup viewers (mails/dumps) switch the instant Settings does. The
  //    emitter receives its own event but the guard makes that a no-op (no loop,
  //    since the listener applies without re-emitting).
  void listen<ThemePref>(THEME_EVENT, ({ payload }) => {
    if (payload !== pref.value) applyPref(payload);
  }).catch(() => {
    // Not in Tauri — single context, nothing to sync.
  });
}
