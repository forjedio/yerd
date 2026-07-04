<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { Minus, Plus, Square, X } from "lucide-vue-next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { hostPlatform } from "@/ipc/client";
import { useTitleBarStyle } from "@/lib/titleBarStyle";

// The window is decorationless (see tauri.conf.json) so we draw our own
// titlebar. It's a `data-tauri-drag-region`, giving native click-drag-to-move
// and double-click-to-zoom for free; the controls below opt out by being their
// own (non-drag) elements.
//
// `title` lets a secondary window (the Mails viewer) reuse this bar with its own
// caption; the optional `actions` slot draws window-scoped buttons on the right
// (outside the drag region, so they stay clickable).
withDefaults(defineProps<{ title?: string }>(), { title: "Yerd" });

// Which control style to draw. Seeded synchronously from the webview user-agent
// so a working control set (crucially, a close button) renders on first paint
// with no flash, then confirmed authoritatively by the daemon's host_platform.
// The UA seed also means the titlebar still works if that IPC call ever fails.
function guessPlatform(): string {
  const ua = navigator.userAgent;
  if (ua.includes("Linux") || ua.includes("X11")) return "linux";
  if (ua.includes("Windows")) return "windows";
  return "macos"; // default also covers macOS ("Macintosh")
}
const platform = ref(guessPlatform());
onMounted(() => {
  hostPlatform()
    .then((p) => (platform.value = p))
    .catch(() => {});
});

// "Automatic" resolves from the host platform; any other preference forces
// that style regardless of host. Unclassified hosts keep today's fallback
// (the left-close / right-min-max layout) so a control set always works.
const { style: stylePref } = useTitleBarStyle();
function platformToStyle(p: string): "macos" | "linux" | "windows" {
  if (p === "macos") return "macos";
  if (p === "windows") return "windows";
  return "linux";
}
const resolved = computed(() =>
  stylePref.value === "auto" ? platformToStyle(platform.value) : stylePref.value,
);

// Controls always target the window this titlebar is mounted in, so the one
// component drives both the main window and the Mails window.
const win = getCurrentWindow();

// Close mirrors the native red button: main.rs intercepts CloseRequested and
// hides to tray rather than quitting, so this is the same close-to-tray gesture.
function close() {
  win.close();
}
function minimize() {
  win.minimize();
}
function toggleMaximize() {
  win.toggleMaximize();
}

// macOS-only: real traffic lights drop to a flat gray while the window is
// unfocused, regaining color only once it's active again. No such tracking
// existed before - Tauri's window API is queried directly (mirrors how
// `lib/theme.ts` layers `onThemeChanged` on top of an initial read).
const focused = ref(true);
let disposed = false;
let unlistenFocus: (() => void) | null = null;
onMounted(() => {
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
});
onUnmounted(() => {
  disposed = true;
  unlistenFocus?.();
});

// Linux/Linux-Reversed/Windows controls, data-driven so the three styles share
// one button template instead of triplicating the markup.
type ControlKind = "close" | "minimize" | "maximize";

const leftControls = computed<ControlKind[]>(() => {
  if (resolved.value === "linux") return ["close"];
  if (resolved.value === "linux-reversed") return ["minimize", "maximize"];
  return [];
});
const rightControls = computed<ControlKind[]>(() => {
  if (resolved.value === "linux") return ["minimize", "maximize"];
  if (resolved.value === "linux-reversed") return ["close"];
  if (resolved.value === "windows") return ["minimize", "maximize", "close"];
  return [];
});

function controlLabel(kind: ControlKind): string {
  if (kind === "close") return "Close";
  if (kind === "minimize") return "Minimize";
  return "Maximize";
}
function controlIcon(kind: ControlKind) {
  if (kind === "close") return X;
  if (kind === "minimize") return Minus;
  return Square;
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
  <header
    data-tauri-drag-region
    class="relative flex h-8 shrink-0 items-center border-b bg-muted px-3 text-foreground dark:bg-card"
    @dblclick="toggleMaximize"
  >
    <!-- macOS: traffic lights (close / minimize / zoom). Colored while the
         window is focused, flat gray otherwise; glyphs revealed on hover. -->
    <div v-if="resolved === 'macos'" class="group flex items-center gap-2">
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

    <!-- Linux: close on the left. Linux (Reversed): minimize/maximize on the
         left instead. Windows: nothing on the left - its controls all sit at
         the far right. -->
    <div v-else-if="leftControls.length" class="flex items-center gap-1">
      <button
        v-for="kind in leftControls"
        :key="kind"
        type="button"
        :aria-label="controlLabel(kind)"
        :class="controlButtonClass(kind)"
        @click="controlAction(kind)"
      >
        <component :is="controlIcon(kind)" :class="controlIconClass(kind)" />
      </button>
    </div>

    <!-- Centered window title (native feel); pointer-events stay with the drag region. -->
    <span
      class="pointer-events-none absolute left-1/2 -translate-x-1/2 text-xs font-medium text-muted-foreground"
    >
      {{ title }}
    </span>

    <!-- Window-scoped actions (e.g. the Mails "clear all" button), right-aligned;
         Linux's minimize/maximize, Linux (Reversed)'s close, and all of
         Windows' controls sit at the far right edge. -->
    <div class="ml-auto flex items-center gap-1">
      <slot name="actions" />
      <button
        v-for="kind in rightControls"
        :key="kind"
        type="button"
        :aria-label="controlLabel(kind)"
        :class="controlButtonClass(kind)"
        @click="controlAction(kind)"
      >
        <component :is="controlIcon(kind)" :class="controlIconClass(kind)" />
      </button>
    </div>
  </header>
</template>
