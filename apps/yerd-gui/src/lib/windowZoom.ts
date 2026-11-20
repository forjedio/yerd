/**
 * Double-click-to-zoom for the window's drag regions (the TitleBar and the
 * PageHeader toolbar). Every window is decorationless, so both surfaces draw
 * their own chrome and must agree on how a double click zooms.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";

import { toggleWindowZoom } from "@/ipc/client";

/**
 * Best guess at the host platform from the webview user-agent. Synchronous, so
 * a working control set (crucially, a close button) renders on first paint
 * with no flash before `host_platform` confirms it authoritatively. It also
 * means the chrome still works if that IPC call ever fails. Anything that is
 * neither Linux nor Windows (including "Macintosh") is treated as macOS.
 */
export function guessPlatform(): string {
  const ua = navigator.userAgent;
  if (ua.includes("Linux") || ua.includes("X11")) return "linux";
  if (ua.includes("Windows")) return "windows";
  return "macos";
}

/** The clickable elements a double click must not zoom through. Mirrors the set
 *  Tauri's drag-region script treats as blocking a drag. */
export const INTERACTIVE_SELECTOR =
  "a, button, input, select, textarea, label, summary, [contenteditable=true]";

/** True when `target` is (or sits inside) a control that owns its own clicks. */
export function isInteractiveTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(INTERACTIVE_SELECTOR) !== null;
}

/**
 * Keep our handler the single owner of double-click-to-zoom. Tauri's
 * drag-region script listens on `document` and runs its own
 * `internal_toggle_maximize` for a drag-region double click: on macOS that is
 * the animated path `zoomWindow` exists to avoid, and on every host it is a
 * second toggle racing ours. Stopping the second press and release before they
 * reach that listener leaves the component's dblclick handler in sole control.
 */
export function stopNativeZoomGesture(event: MouseEvent): void {
  if (event.detail === 2) event.stopPropagation();
}

/**
 * Zoom/restore the current window. macOS goes through `toggle_window_zoom`:
 * our windows are decorationless, and for those Tauri's `toggleMaximize`
 * animates the frame with redraw suppressed - the window appears to slide to
 * the screen corner at its old size, then snap to full size when the animation
 * ends. The command runs the same animation with the window actually drawn.
 *
 * Resolves `true` when the toggle call succeeded and `false` when it threw
 * (the error is swallowed). `getCurrentWindow()` is called lazily so importing
 * this module never touches Tauri.
 */
export async function zoomWindow(platform: string): Promise<boolean> {
  try {
    if (platform === "macos") {
      await toggleWindowZoom();
    } else {
      await getCurrentWindow().toggleMaximize();
    }
    return true;
  } catch {
    return false;
  }
}
