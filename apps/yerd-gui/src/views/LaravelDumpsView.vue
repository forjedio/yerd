<script setup lang="ts">
import { AppWindow, Pause, Play, SlidersHorizontal } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import DumpsCapturePill from "@/components/dumps/DumpsCapturePill.vue";
import DumpsRequestDetail from "@/components/dumps/DumpsRequestDetail.vue";
import DumpsRequestList from "@/components/dumps/DumpsRequestList.vue";
import DumpsSetupDialog from "@/components/dumps/DumpsSetupDialog.vue";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDumps } from "@/composables/useDumps";
import { useWideLayout } from "@/composables/useWideLayout";
import { showDumpsWindow } from "@/ipc/client";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { toastOnError } from "@/lib/toastActions";
import { cn } from "@/lib/utils";

const router = useRouter();
const wide = useWideLayout();
const dumps = useDumps({ pollWhileHidden: false, follow: () => wide.value });
const { busy, category, query, requests, selected, selectedKey, status, visible } = dumps;

onUnmounted(registerViewActions({ refresh: () => void dumps.refresh() }));

const enabled = computed(() => status.value?.enabled ?? false);
const listed = computed(() => (wide.value ? visible.value : requests.value));

const setupOpen = ref(false);
const drawerOpen = ref(false);

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

watch(selectedKey, (key) => {
  if (!key) drawerOpen.value = false;
});

watch(drawerOpen, (open) => {
  if (!open && !wide.value) query.value = "";
});

function onSelect(key: string): void {
  dumps.select(key);
  if (!wide.value) drawerOpen.value = true;
}

function changePort(): void {
  setupOpen.value = false;
  void router.push("/general");
}

async function openWindow(): Promise<void> {
  await toastOnError("Couldn't open the dumps window", () => showDumpsWindow());
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Dumps" docs="/guide/laravel-dumps">
      <template #actions>
        <DumpsCapturePill :status="status" :compact="!wide" />
        <Button
          variant="outline"
          :size="wide ? 'sm' : 'icon'"
          :class="cn(!wide && 'size-8')"
          :disabled="!status || busy === 'enabled'"
          :aria-label="enabled ? 'Pause' : 'Resume'"
          :title="enabled ? 'Pause interception' : 'Resume interception'"
          data-testid="pause-resume"
          @click="dumps.setEnabled(!enabled)"
        >
          <Spinner v-if="busy === 'enabled'" class="text-current" />
          <Pause v-else-if="enabled" />
          <Play v-else />
          <span v-if="wide">{{ enabled ? "Pause" : "Resume" }}</span>
        </Button>
        <Button
          variant="outline"
          :size="wide ? 'sm' : 'icon'"
          :class="cn(!wide && 'size-8')"
          aria-label="Setup"
          title="Dump setup"
          @click="setupOpen = true"
        >
          <SlidersHorizontal />
          <span v-if="wide">Setup</span>
        </Button>
        <Button
          variant="outline"
          size="icon"
          class="size-8"
          aria-label="Open in window"
          title="Open in a separate window"
          @click="openWindow"
        >
          <AppWindow />
        </Button>
      </template>
    </PageHeader>

    <div class="flex min-h-0 flex-1" :data-layout="wide ? 'wide' : 'compact'">
      <div
        :class="
          wide && requests.length > 0
            ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-2.5'
            : 'flex min-w-0 flex-1 flex-col pl-4 pt-2.5'
        "
      >
        <DumpsRequestList
          class="flex-1"
          :layout="wide ? 'list' : 'table'"
          :requests="listed"
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

      <div :class="wide && requests.length > 0 ? 'min-w-0 flex-1 border-l' : 'hidden'" data-testid="detail-column">
        <InspectorFrame
          :mode="wide ? 'panel' : 'drawer'"
          :shown="wide ? requests.length > 0 : drawerOpen && !!selected"
          label="Request"
          testid="dumps-detail"
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

    <DumpsSetupDialog
      v-model:open="setupOpen"
      :status="status"
      :busy="busy"
      @set-enabled="dumps.setEnabled"
      @set-feature="dumps.setFeature"
      @change-port="changePort"
    />
  </div>
</template>
