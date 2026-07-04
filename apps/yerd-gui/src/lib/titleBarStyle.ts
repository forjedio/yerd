/**
 * Title bar style store: a reactive user preference, persisted host-side
 * (`gui-settings.json` via the `get_title_bar_style`/`set_title_bar_style`
 * commands - mirrors the tray icon variant setting) and broadcast live to
 * every open window.
 *
 * Unlike the theme preference (`lib/theme.ts`, `localStorage`), the source of
 * truth here is the host settings file, since we mirror the tray-icon storage
 * model. But the title bar - unlike the tray icon - is drawn once per window
 * (main/mails/dumps), so a change still needs the same cross-window broadcast
 * `theme.ts` uses: each webview is a separate JS context, so one window's
 * change is invisible to the others without it.
 */
import { emit, listen } from "@tauri-apps/api/event";
import { ref } from "vue";

import { getTitleBarStyle, setTitleBarStyle as ipcSetTitleBarStyle } from "@/ipc/client";
import type { TitleBarStyle } from "@/ipc/types";

/** Cross-window broadcast: each webview (main, mails, dumps) is a separate JS
 *  context + DOM, so a change in one is published to the others. */
const TITLE_BAR_STYLE_EVENT = "yerd:title-bar-style-changed";

/** The user's preference (reactive, persisted host-side). */
const style = ref<TitleBarStyle>("auto");

/** Bumped on every `setTitleBarStyle` call; lets a call tell whether a *newer*
 *  one has since taken over, so two overlapping calls (the user changing the
 *  selector again before the first IPC round-trip lands) can't have the
 *  older one's rollback or broadcast clobber the newer one's result. */
let latestCall = 0;

/** Set + persist the preference, then broadcast to the app's other windows
 *  (mails/dumps) so they switch in lockstep. Optimistic: the ref updates
 *  immediately, but a failed IPC call rolls it back and does NOT broadcast -
 *  other windows must never see a value that wasn't actually persisted. */
export async function setTitleBarStyle(next: TitleBarStyle): Promise<void> {
  const call = ++latestCall;
  const previous = style.value;
  style.value = next;
  try {
    await ipcSetTitleBarStyle(next);
  } catch (e) {
    if (call === latestCall) style.value = previous;
    throw e;
  }
  if (call === latestCall) {
    void emit(TITLE_BAR_STYLE_EVENT, next).catch(() => {
      // Not in Tauri (unit/dev) - nothing to broadcast.
    });
  }
}

/** Reactive accessor for views/components (the General tab's selector and
 *  `TitleBar.vue` itself). */
export function useTitleBarStyle() {
  return { style, setTitleBarStyle };
}

/** Load the persisted preference and wire up cross-window sync. Call once at
 *  boot, alongside `initTheme()`. */
export function initTitleBarStyle(): void {
  // The initial fetch and the broadcast subscription are two independent
  // round-trips with no ordering guarantee - if another window changes the
  // preference while this fetch is still in flight, its broadcast could
  // otherwise arrive first and then be clobbered by the now-stale fetch
  // result. Once any broadcast has been seen, the fetch no longer applies.
  let sawBroadcast = false;

  void listen<TitleBarStyle>(TITLE_BAR_STYLE_EVENT, ({ payload }) => {
    sawBroadcast = true;
    style.value = payload;
  }).catch(() => {
    // Not in Tauri - single context, nothing to sync.
  });

  void getTitleBarStyle()
    .then((s) => {
      if (!sawBroadcast) style.value = s;
    })
    .catch(() => {
      // Keep the "auto" default if the host call fails.
    });
}
