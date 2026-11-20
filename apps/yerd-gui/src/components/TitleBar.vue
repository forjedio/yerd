<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Copy, Minus, Plus, Square, X } from "lucide-vue-next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { hostPlatform, setGuiMaximized } from "@/ipc/client";
import { useTitleBarStyle } from "@/lib/titleBarStyle";
import {
  guessPlatform,
  isInteractiveTarget,
  stopNativeZoomGesture,
  zoomWindow,
} from "@/lib/windowZoom";
import { supportsReliableMaximizedState } from "@/lib/windowState";

// The window is decorationless (see tauri.conf.json) so we draw our own window
// controls, always on the left whatever the style. The root is a
// `data-tauri-drag-region`, giving native click-drag-to-move for free; the
// controls opt out by being their own (non-drag) elements. Double-click-to-zoom
// is handled here rather than by the drag region - see `lib/windowZoom.ts`.
//
// `variant="bar"` (default) is the 32px bar secondary windows (Mails, Dumps,
// Welcome) draw across their top: controls left, a centred `title`, and an
// optional `actions` slot on the right (outside the drag region's own element,
// so they stay clickable). `variant="sidebar"` is the main window's: controls
// only, no title, border or background, placed at the sidebar's top; its
// default slot (the sidebar's brand block) sits below the controls inside the
// same deep drag region and zoom handlers.
withDefaults(defineProps<{ title?: string; variant?: "bar" | "sidebar" }>(), {
  title: "Yerd",
  variant: "bar",
});

// Which control style to draw. Seeded synchronously from the webview user-agent
// (see `guessPlatform`), then confirmed authoritatively by host_platform.
const platform = ref(guessPlatform());
onMounted(() => {
  hostPlatform()
    .then((p) => (platform.value = p))
    .catch(() => {});
});

// "Automatic" resolves from the host platform; any other preference forces
// that style regardless of host. Unclassified hosts fall back to the Linux
// layout so a control set always works.
const { style: stylePref } = useTitleBarStyle();
function platformToStyle(p: string): "macos" | "linux" | "windows" {
  if (p === "macos") return "macos";
  if (p === "windows") return "windows";
  return "linux";
}
const resolved = computed(() =>
  stylePref.value === "auto" ? platformToStyle(platform.value) : stylePref.value,
);
const maximizeStateSupported = computed(() =>
  supportsReliableMaximizedState(platform.value),
);

// Controls always target the window this titlebar is mounted in, so the one
// component drives both the main window and the Mails window.
const win = getCurrentWindow();
let disposed = false;
let lastPersistedMaximized: boolean | null = null;
let pendingMaximized: boolean | null = null;
let maximizedPersistenceInFlight = false;
let maximizedPersistenceRetry: number | null = null;
let maximizedRefreshId = 0;
let maximizedRefreshTimer: number | null = null;

// Close mirrors the native red button: main.rs intercepts CloseRequested and
// hides to tray rather than quitting, so this is the same close-to-tray gesture.
function close() {
  win.close();
}
function minimize() {
  win.minimize();
}
const maximized = ref(false);

/** Coalesce main-window maximize writes and retry transient IPC failures. */
function queueMaximizedPersistence(value: boolean): void {
  if (win.label !== "main" || disposed || !maximizeStateSupported.value) return;
  pendingMaximized = value;
  if (!maximizedPersistenceInFlight) {
    void flushMaximizedPersistence();
  }
}

async function flushMaximizedPersistence(): Promise<void> {
  if (maximizedPersistenceInFlight) return;
  maximizedPersistenceInFlight = true;
  if (maximizedPersistenceRetry !== null) {
    window.clearTimeout(maximizedPersistenceRetry);
    maximizedPersistenceRetry = null;
  }
  while (pendingMaximized !== null && !disposed) {
    const value = pendingMaximized;
    pendingMaximized = null;
    if (lastPersistedMaximized === value) continue;
    try {
      await setGuiMaximized(value);
      lastPersistedMaximized = value;
    } catch {
      if (pendingMaximized === null) pendingMaximized = value;
      break;
    }
  }
  maximizedPersistenceInFlight = false;
  if (!disposed && pendingMaximized !== null) {
    maximizedPersistenceRetry = window.setTimeout(() => {
      maximizedPersistenceRetry = null;
      void flushMaximizedPersistence();
    }, 1000);
  }
}

function updateMaximized(value: boolean): void {
  maximized.value = value;
  if (maximizeStateSupported.value) {
    document.documentElement.classList.toggle("window-maximized", value);
    queueMaximizedPersistence(value);
  } else {
    document.documentElement.classList.remove("window-maximized");
  }
}

function refreshMaximized(): void {
  const requestId = ++maximizedRefreshId;
  if (!maximizeStateSupported.value) {
    maximized.value = false;
    document.documentElement.classList.remove("window-maximized");
    return;
  }
  win
    .isMaximized()
    .then((value) => {
      if (disposed || requestId !== maximizedRefreshId) return;
      updateMaximized(value);
    })
    .catch(() => {});
}

/**
 * Collapse a resize gesture's event storm into a single state read. GTK/WebKit
 * and Windows emit resize events continuously through a live edge-drag, and
 * every `refreshMaximized` is an IPC round trip. Only the WM-driven path is
 * debounced: a control click or titlebar double-click still refreshes at once.
 */
function scheduleMaximizedRefresh(): void {
  if (disposed) return;
  if (maximizedRefreshTimer !== null) window.clearTimeout(maximizedRefreshTimer);
  maximizedRefreshTimer = window.setTimeout(() => {
    maximizedRefreshTimer = null;
    refreshMaximized();
  }, 150);
}

/**
 * Zoom/restore the window (see `zoomWindow`) and refresh the maximized state.
 * The macOS zoom command schedules its animation and returns, so there is no
 * settled frame to read afterwards: that branch leaves the refresh to
 * `onResized`'s debounce rather than sampling a frame mid-flight. A failed
 * toggle refreshes nothing.
 */
async function toggleMaximize(): Promise<void> {
  const ok = await zoomWindow(platform.value);
  if (!ok || platform.value === "macos") return;
  refreshMaximized();
}

/** Double-clicking the bar zooms, including the gaps around the controls, but a
 *  double click *on* a control (a traffic light, a slotted action) belongs to
 *  that control alone. */
function onTitleBarDoubleClick(event: MouseEvent): void {
  if (isInteractiveTarget(event.target)) return;
  void toggleMaximize();
}

// macOS-only: real traffic lights drop to a flat gray while the window is
// unfocused, regaining color only once it's active again. No such tracking
// existed before - Tauri's window API is queried directly (mirrors how
// `lib/theme.ts` layers `onThemeChanged` on top of an initial read).
const focused = ref(true);
let unlistenFocus: (() => void) | null = null;
let unlistenResize: (() => void) | null = null;
onMounted(() => {
  refreshMaximized();
  win.isFocused()
    .then((f) => (focused.value = f))
    .catch(() => {});
  win
    .onFocusChanged(({ payload }) => (focused.value = payload))
    .then((unlisten) => {
      // The component may have unmounted while this registration was still in
      // flight (e.g. the Welcome route swapping out for AppShell's titlebar) -
      // in that case there's no later onUnmounted to call it, so do it now.
      if (disposed) {
        unlisten();
      } else {
        unlistenFocus = unlisten;
      }
    })
    .catch(() => {});
  win
    .onResized(() => scheduleMaximizedRefresh())
    .then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlistenResize = unlisten;
      }
    })
    .catch(() => {});
});
onUnmounted(() => {
  disposed = true;
  maximizedRefreshId += 1;
  if (maximizedPersistenceRetry !== null) {
    window.clearTimeout(maximizedPersistenceRetry);
    maximizedPersistenceRetry = null;
  }
  if (maximizedRefreshTimer !== null) {
    window.clearTimeout(maximizedRefreshTimer);
    maximizedRefreshTimer = null;
  }
  unlistenFocus?.();
  unlistenResize?.();
  document.documentElement.classList.remove("window-maximized");
});

// Linux/Windows controls, data-driven so both styles share one button template.
// Both sit on the left: Linux in close/minimize/maximize order, Windows in its
// native minimize/maximize/close order.
type ControlKind = "close" | "minimize" | "maximize";

const controls = computed<ControlKind[]>(() => {
  if (resolved.value === "linux") return ["close", "minimize", "maximize"];
  if (resolved.value === "windows") return ["minimize", "maximize", "close"];
  return [];
});

function controlLabel(kind: ControlKind): string {
  if (kind === "close") return "Close";
  if (kind === "minimize") return "Minimize";
  return maximized.value ? "Restore" : "Maximize";
}
function controlIcon(kind: ControlKind) {
  if (kind === "close") return X;
  if (kind === "minimize") return Minus;
  return maximized.value ? Copy : Square;
}
function controlIconClass(kind: ControlKind): string {
  return kind === "maximize" ? "size-3.5" : "size-4";
}
function controlAction(kind: ControlKind): () => void {
  if (kind === "close") return close;
  if (kind === "minimize") return minimize;
  return toggleMaximize;
}
// Windows convention: the close button gets a distinct red hover; the other
// two share the neutral hover already used for Linux controls.
function controlButtonClass(kind: ControlKind): string {
  const base =
    "flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors";
  if (resolved.value === "windows" && kind === "close") {
    return `${base} hover:bg-red-600 hover:text-white`;
  }
  return `${base} hover:bg-black/10 dark:hover:bg-white/10`;
}
</script>

<template>
  <component
    :is="variant === 'bar' ? 'header' : 'div'"
    :data-tauri-drag-region="variant === 'sidebar' ? 'deep' : ''"
    :class="
      variant === 'bar'
        ? 'relative flex h-8 w-full shrink-0 items-center border-b bg-muted px-3 text-foreground dark:bg-card'
        : 'flex shrink-0 flex-col text-foreground'
    "
    @mousedown="stopNativeZoomGesture"
    @mouseup="stopNativeZoomGesture"
    @dblclick="onTitleBarDoubleClick"
  >
    <!-- `bar`: the bare drag attribute (self-only), with the centred title
         pointer-events-none so drags fall through to the header. `sidebar`:
         "deep", so the gaps around the controls and the slotted brand block
         drag the window too; buttons still block the drag. -->
    <div
      :class="
        variant === 'sidebar' ? 'flex h-[50px] items-center px-[18px]' : 'contents'
      "
    >
      <!-- macOS: traffic lights (close / minimize / zoom). Colored while the
           window is focused, flat gray otherwise; glyphs revealed on hover.
           Both control sets sit at z-[46], above an inspector drawer's
           backdrop and panel, so the window can be closed or zoomed while a
           drawer is open. -->
      <div v-if="resolved === 'macos'" class="group relative z-[46] flex items-center gap-2">
        <button
          type="button"
          aria-label="Close"
          class="flex size-3 items-center justify-center rounded-full text-black/60 transition-colors"
          :class="focused ? 'bg-[#ff5f57]' : 'bg-[#d3d3d3] dark:bg-[#4a4a4c]'"
          @click="close"
        >
          <X class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
        </button>
        <button
          type="button"
          aria-label="Minimize"
          class="flex size-3 items-center justify-center rounded-full text-black/60 transition-colors"
          :class="focused ? 'bg-[#febc2e]' : 'bg-[#d3d3d3] dark:bg-[#4a4a4c]'"
          @click="minimize"
        >
          <Minus class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
        </button>
        <button
          type="button"
          aria-label="Zoom"
          class="flex size-3 items-center justify-center rounded-full text-black/60 transition-colors"
          :class="focused ? 'bg-[#28c840]' : 'bg-[#d3d3d3] dark:bg-[#4a4a4c]'"
          @click="toggleMaximize"
        >
          <Plus class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
        </button>
      </div>

      <!-- Linux: close / minimize / maximize. Windows: minimize / maximize / close. -->
      <div v-else-if="controls.length" class="relative z-[46] flex items-center gap-1">
        <button
          v-for="kind in controls"
          :key="kind"
          type="button"
          :aria-label="controlLabel(kind)"
          :class="controlButtonClass(kind)"
          @click="controlAction(kind)()"
        >
          <component :is="controlIcon(kind)" :class="controlIconClass(kind)" />
        </button>
      </div>
    </div>

    <template v-if="variant === 'bar'">
      <!-- Centered window title (native feel); pointer-events stay with the drag region. -->
      <span
        class="pointer-events-none absolute left-1/2 -translate-x-1/2 text-xs font-medium text-muted-foreground"
      >
        {{ title }}
      </span>

      <!-- Window-scoped actions (e.g. the Mails "clear all" button), right-aligned. -->
      <div class="ml-auto flex items-center gap-1">
        <slot name="actions" />
      </div>
    </template>
    <slot v-else />
  </component>
</template>
