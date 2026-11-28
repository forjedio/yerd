<script setup lang="ts">
import { AppWindow } from "lucide-vue-next";
import { ref, watch } from "vue";

import DumpsRequestDetail from "@/components/dumps/DumpsRequestDetail.vue";
import DumpsRequestList from "@/components/dumps/DumpsRequestList.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import { useDumps } from "@/composables/useDumps";
import { showDumpsWindow } from "@/ipc/client";
import { toastOnError } from "@/lib/toastActions";

/**
 * The Live feed tab: the dumps stream (dumps, queries, logs, jobs...) scoped
 * to the Insights site, with the request list beside its detail on wide
 * windows and a drawer on compact ones. Mounted only while the tab is open, so
 * the stream polls only then. The page's Pause holds it still. The same feed
 * pops out into the Dumps window.
 */
const props = defineProps<{ site: string | null; tld: string; wide: boolean; paused: boolean }>();

const dumps = useDumps({
  pollWhileHidden: false,
  follow: () => props.wide,
  site: () => props.site,
  tld: () => props.tld,
  paused: () => props.paused,
});
const { busy, category, query, requests, selected, selectedKey, status, visible } = dumps;

const drawerOpen = ref(false);

watch(
  () => props.wide,
  (isWide) => {
    if (isWide) drawerOpen.value = false;
  },
);

watch(selectedKey, (key) => {
  if (!key) drawerOpen.value = false;
});

function onSelect(key: string): void {
  dumps.select(key);
  if (!props.wide) drawerOpen.value = true;
}

async function openWindow(): Promise<void> {
  await toastOnError("Couldn't open the dumps window", () => showDumpsWindow());
}

defineExpose({ refresh: () => dumps.refresh() });
</script>

<template>
  <div class="flex min-h-0 flex-1" data-testid="live-feed">
    <div
      :class="
        wide && requests.length > 0
          ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-2.5'
          : 'flex min-w-0 flex-1 flex-col pl-4 pt-2.5'
      "
    >
      <div class="flex justify-end pr-4">
        <Button variant="ghost" size="sm" title="Open the live feed in a separate window" @click="openWindow">
          <AppWindow />
          Pop out
        </Button>
      </div>
      <DumpsRequestList
        class="flex-1"
        :layout="wide ? 'list' : 'table'"
        :requests="wide ? visible : requests"
        :total="requests.length"
        :selected="selectedKey"
        :status="status"
        :busy="busy"
        :query="query"
        :category="category"
        @select="onSelect"
        @toggle-persist="dumps.setPersist"
        @clear="dumps.clear"
        @resume="dumps.setEnabled(true)"
        @clear-filter="dumps.clearFilter"
      />
    </div>
    <div :class="wide && requests.length > 0 ? 'min-w-0 flex-1 border-l' : 'hidden'">
      <InspectorFrame
        :mode="wide ? 'panel' : 'drawer'"
        :shown="wide ? requests.length > 0 : drawerOpen && !!selected"
        label="Request"
        testid="live-detail"
        wide-drawer
        @close="drawerOpen = false"
      >
        <DumpsRequestDetail
          v-model:category="category"
          v-model:query="query"
          :request="selected"
          :closable="!wide"
          @close="drawerOpen = false"
        />
      </InspectorFrame>
    </div>
  </div>
</template>
