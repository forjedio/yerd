<script setup lang="ts">
import { computed, inject } from "vue";
import { ArrowUpRight, Search } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import { usePlatform } from "@/composables/usePlatform";
import { openInBrowser } from "@/ipc/client";
import { formatChord } from "@/lib/shortcuts/format";
import { isMac } from "@/lib/shortcuts/platform";
import { OPEN_PALETTE_KEY } from "@/lib/shortcuts/registry";
import {
  guessPlatform,
  isInteractiveTarget,
  stopNativeZoomGesture,
  zoomWindow,
} from "@/lib/windowZoom";

/**
 * The 56px page toolbar: title on the left, a command-search trigger in the
 * middle (opens the app shell's command palette, rendered only when the shell
 * provides `OPEN_PALETTE_KEY`), then Docs and the page's own `actions` slot on
 * the right.
 *
 * It is also the main window's drag region. The attribute is "deep" so empty
 * space and the title drag too, while buttons and the search trigger still
 * block it. Double-click zooms the window exactly as the TitleBar does (see
 * `lib/windowZoom.ts`); the OS resize event then refreshes the TitleBar's
 * maximized state.
 *
 * `docs` is a path under the documentation site (e.g. "/guide/mail"); when set,
 * a Docs button opens that page in the user's browser.
 */
const props = defineProps<{ title: string; docs?: string }>();

const openPalette = inject(OPEN_PALETTE_KEY, null);
const paletteHint = formatChord({ mod: true, key: "k" }, isMac());
const { platform: hostPlatform } = usePlatform();
const platform = computed(() => hostPlatform.value || guessPlatform());

function openDocs(): void {
  if (props.docs) void openInBrowser(`https://yerd.app${props.docs}`);
}

function onDoubleClick(event: MouseEvent): void {
  if (isInteractiveTarget(event.target)) return;
  void zoomWindow(platform.value);
}
</script>

<template>
  <header
    data-tauri-drag-region="deep"
    class="sticky top-0 z-10 flex h-14 shrink-0 items-center gap-3 border-b bg-background px-6"
    @mousedown="stopNativeZoomGesture"
    @mouseup="stopNativeZoomGesture"
    @dblclick="onDoubleClick"
  >
    <h2 class="min-w-0 flex-1 basis-0 truncate text-base font-semibold tracking-[-0.2px]">
      {{ title }}
    </h2>
    <button
      v-if="openPalette"
      type="button"
      aria-label="Search or run a command"
      class="flex h-8 min-w-0 max-w-[420px] flex-1 items-center gap-2 rounded-[7px] border bg-muted px-2.5 text-[13px] text-faint transition-colors hover:text-muted-foreground"
      @click="openPalette()"
    >
      <Search class="size-3.5 shrink-0" />
      <span class="min-w-0 flex-1 truncate text-left">
        Search sites, services, or run a command…
      </span>
      <kbd
        class="shrink-0 rounded border bg-background px-1.5 font-sans text-[11px] leading-[18px] text-muted-foreground"
        >{{ paletteHint }}</kbd
      >
    </button>
    <div class="flex flex-1 basis-0 items-center justify-end gap-2">
      <Button v-if="docs" variant="outline" size="sm" @click="openDocs">
        Docs
        <ArrowUpRight class="opacity-70" />
      </Button>
      <slot name="actions" />
    </div>
  </header>
</template>
