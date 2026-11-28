<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Pin, PinOff } from "lucide-vue-next";
import { onMounted, onUnmounted, ref } from "vue";

import DumpsCapturePill from "@/components/dumps/DumpsCapturePill.vue";
import DumpsRequestDetail from "@/components/dumps/DumpsRequestDetail.vue";
import DumpsRequestList from "@/components/dumps/DumpsRequestList.vue";
import TitleBar from "@/components/TitleBar.vue";
import { useDumps } from "@/composables/useDumps";
import { INSIGHTS_SITE_EVENT, storedInsightsSite } from "@/composables/useInsights";
import { useToast } from "@/composables/useToast";
import { CATEGORY_CHIPS } from "@/lib/dumps";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { cn } from "@/lib/utils";

/**
 * The separate Dumps window (opened from the Insights Live feed, the tray, ⇧⌘D
 * and a site's menu): the Live feed's request list and detail, always side by
 * side, streaming while the window is hidden. It shows the site picked in
 * Insights, read from storage on open and then followed through the
 * `yerd:insights-site-changed` event. The title bar carries the site, the
 * capture pill and pin on top; setup stays in the main window.
 */
const toast = useToast();
const site = ref<string | null>(storedInsightsSite());
const dumps = useDumps({ pollWhileHidden: true, follow: () => true, site: () => site.value });
const { busy, category, query, requests, selected, selectedKey, status, visible } = dumps;

let unlisten: UnlistenFn | null = null;
let disposed = false;

onMounted(async () => {
  try {
    const off = await listen<string>(INSIGHTS_SITE_EVENT, ({ payload }) => {
      if (typeof payload === "string" && payload !== site.value) {
        site.value = payload;
        dumps.select(null);
      }
    });
    if (disposed) off();
    else unlisten = off;
  } catch {
    unlisten = null;
  }
});

onUnmounted(() => {
  disposed = true;
  unlisten?.();
});

const alwaysOnTop = ref(false);
const detail = ref<InstanceType<typeof DumpsRequestDetail> | null>(null);

async function toggleAlwaysOnTop(): Promise<void> {
  const next = !alwaysOnTop.value;
  try {
    await getCurrentWindow().setAlwaysOnTop(next);
    alwaysOnTop.value = next;
  } catch (e) {
    toast.error("Couldn't pin window", String(e));
  }
}

function cycleCategory(delta: number): void {
  const i = CATEGORY_CHIPS.findIndex((c) => c.key === category.value);
  const next = CATEGORY_CHIPS[(i + delta + CATEGORY_CHIPS.length) % CATEGORY_CHIPS.length];
  if (next) category.value = next.key;
}

onUnmounted(
  registerViewActions({
    find: () => void detail.value?.openFilter(),
    refresh: () => void dumps.refresh(),
    prevTab: () => cycleCategory(-1),
    nextTab: () => cycleCategory(1),
  }),
);

const ICON_BUTTON =
  "flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
</script>

<template>
  <div class="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
    <TitleBar :title="site ? `Live feed · ${site}` : 'Live feed'">
      <template #actions>
        <DumpsCapturePill :status="status" compact />
        <button
          type="button"
          :class="cn(ICON_BUTTON, alwaysOnTop && 'bg-brand/15 text-brand hover:bg-brand/20 hover:text-brand')"
          :aria-pressed="alwaysOnTop"
          :aria-label="alwaysOnTop ? 'Unpin window' : 'Keep window on top'"
          :title="alwaysOnTop ? 'Unpin window' : 'Keep window on top'"
          @click="toggleAlwaysOnTop"
        >
          <PinOff v-if="alwaysOnTop" class="size-3.5" />
          <Pin v-else class="size-3.5" />
        </button>
      </template>
    </TitleBar>

    <div class="flex min-h-0 flex-1">
      <div class="flex w-[320px] shrink-0 flex-col border-r pl-4 pt-2.5">
        <DumpsRequestList
          class="flex-1"
          layout="list"
          :requests="visible"
          :total="requests.length"
          :selected="selectedKey"
          :status="status"
          :busy="busy"
          :query="query"
          :category="category"
          @select="dumps.select"
          @toggle-persist="dumps.setPersist"
          @clear="dumps.clear"
          @resume="dumps.setEnabled(true)"
          @clear-filter="dumps.clearFilter"
        />
      </div>

      <div class="min-w-0 flex-1 overflow-y-auto">
        <DumpsRequestDetail ref="detail" v-model:category="category" v-model:query="query" :request="selected" />
      </div>
    </div>
  </div>
</template>
