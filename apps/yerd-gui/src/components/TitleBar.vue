<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Minus, Plus, Square, X } from "lucide-vue-next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { hostPlatform } from "@/ipc/client";

// The window is decorationless (see tauri.conf.json) so we draw our own
// titlebar. It's a `data-tauri-drag-region`, giving native click-drag-to-move
// and double-click-to-zoom for free; the controls below opt out by being their
// own (non-drag) elements.
//
// Controls follow the host OS convention: macOS draws traffic lights on the
// left; Linux (Pantheon/GNOME style) puts a close button on the left and
// minimize/maximize at the far right.
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
// macOS gets traffic lights; every other host gets the left-close / right-min-max
// layout. Keyed off `isMac` (not an `isLinux` allowlist) so an unclassified host
// still renders working controls.
const isMac = computed(() => platform.value === "macos");
onMounted(() => {
  hostPlatform()
    .then((p) => (platform.value = p))
    .catch(() => {});
});

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
</script>

<template>
  <header
    data-tauri-drag-region
    class="relative flex h-8 shrink-0 items-center border-b bg-muted px-3 text-foreground dark:bg-card"
    @dblclick="toggleMaximize"
  >
    <!-- macOS: traffic lights (close / minimize / zoom), glyphs revealed on hover. -->
    <div v-if="isMac" class="group flex items-center gap-2">
      <button
        type="button"
        aria-label="Close"
        class="flex size-3 items-center justify-center rounded-full bg-[#ff5f57] text-black/60 transition-colors hover:bg-[#ff5f57]"
        @click="close"
      >
        <X class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
      </button>
      <button
        type="button"
        aria-label="Minimize"
        class="flex size-3 items-center justify-center rounded-full bg-[#febc2e] text-black/60 transition-colors"
        @click="minimize"
      >
        <Minus class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
      </button>
      <button
        type="button"
        aria-label="Zoom"
        class="flex size-3 items-center justify-center rounded-full bg-[#28c840] text-black/60 transition-colors"
        @click="toggleMaximize"
      >
        <Plus class="size-2 opacity-0 group-hover:opacity-100" stroke-width="3" />
      </button>
    </div>

    <!-- Non-macOS (Linux/Pantheon convention, and any other host): close on the
         left. `v-else` rather than `v-else-if="isLinux"` so a host that isn't
         classified still gets a working close button. -->
    <button
      v-else
      type="button"
      aria-label="Close"
      class="flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-black/10 dark:hover:bg-white/10"
      @click="close"
    >
      <X class="size-4" />
    </button>

    <!-- Centered window title (native feel); pointer-events stay with the drag region. -->
    <span
      class="pointer-events-none absolute left-1/2 -translate-x-1/2 text-xs font-medium text-muted-foreground"
    >
      {{ title }}
    </span>

    <!-- Window-scoped actions (e.g. the Mails "clear all" button), right-aligned;
         on Linux the minimize/maximize controls sit at the far right edge. -->
    <div class="ml-auto flex items-center gap-1">
      <slot name="actions" />
      <template v-if="!isMac">
        <button
          type="button"
          aria-label="Minimize"
          class="flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-black/10 dark:hover:bg-white/10"
          @click="minimize"
        >
          <Minus class="size-4" />
        </button>
        <button
          type="button"
          aria-label="Maximize"
          class="flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-black/10 dark:hover:bg-white/10"
          @click="toggleMaximize"
        >
          <Square class="size-3.5" />
        </button>
      </template>
    </div>
  </header>
</template>
