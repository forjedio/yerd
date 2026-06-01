<script setup lang="ts">
import { Minus, Plus, X } from "lucide-vue-next";
import { getCurrentWindow } from "@tauri-apps/api/window";

// The window is decorationless (see tauri.conf.json) so we draw our own
// titlebar. It's a `data-tauri-drag-region`, giving native click-drag-to-move
// and double-click-to-zoom for free; the controls below opt out by being their
// own (non-drag) elements. Identical on macOS and Linux by design.
const win = getCurrentWindow();

// Close mirrors the native red button: main.rs intercepts CloseRequested and
// hides to tray rather than quitting, so this is the same close-to-tray gesture.
function close() {
  void win.close();
}
function minimize() {
  void win.minimize();
}
function toggleMaximize() {
  void win.toggleMaximize();
}
</script>

<template>
  <header
    data-tauri-drag-region
    class="relative flex h-8 shrink-0 items-center border-b bg-muted px-3 text-foreground dark:bg-card"
    @dblclick="toggleMaximize"
  >
    <!-- Traffic lights: close / minimize / zoom, glyphs revealed on hover. -->
    <div class="group flex items-center gap-2">
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

    <!-- Centered window title (native feel); pointer-events stay with the drag region. -->
    <span
      class="pointer-events-none absolute left-1/2 -translate-x-1/2 text-xs font-medium text-muted-foreground"
    >
      Yerd
    </span>
  </header>
</template>
