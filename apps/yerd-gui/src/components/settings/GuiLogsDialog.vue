<script setup lang="ts">
import { Copy, RefreshCw } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import { useToast } from "@/composables/useToast";
import { getGuiLogs, IpcError } from "@/ipc/client";
import type { GuiLogs } from "@/ipc/types";
import { copyWithToast } from "@/lib/toastActions";

/**
 * The GUI and daemon logs from About ▸ Troubleshooting. While open it re-reads
 * both every 2s so a live daemon-start trail streams in; a failed poll stays
 * quiet and only the first read or a manual Refresh toasts.
 */
const props = defineProps<{ open: boolean }>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const POLL_MS = 2000;

const toast = useToast();
const logs = ref<GuiLogs | null>(null);
const activeTab = ref<"gui" | "daemon">("gui");
let timer: ReturnType<typeof setInterval> | undefined;

const tabOptions = [
  { value: "gui", label: "GUI" },
  { value: "daemon", label: "Daemon" },
] as const;

const activeLines = computed(() =>
  activeTab.value === "gui" ? (logs.value?.guiLog ?? []) : (logs.value?.daemonLog ?? []),
);
const activePath = computed(() =>
  activeTab.value === "gui" ? logs.value?.guiPath : logs.value?.daemonPath,
);

async function fetchLogs(silent = false): Promise<void> {
  try {
    logs.value = await getGuiLogs();
  } catch (e) {
    if (!silent) toast.error("Couldn't read logs", (e as IpcError).message);
  }
}

function stop(): void {
  if (timer) clearInterval(timer);
  timer = undefined;
}

function start(): void {
  stop();
  void fetchLogs();
  timer = setInterval(() => void fetchLogs(true), POLL_MS);
}

watch(
  () => props.open,
  (open) => {
    if (open) start();
    else stop();
  },
  { immediate: true },
);

onUnmounted(stop);

function onTab(value: string): void {
  if (value === "gui" || value === "daemon") activeTab.value = value;
}

async function copyActive(): Promise<void> {
  await copyWithToast(activeLines.value.join("\n"), {
    title: "Copied logs",
    detail: "Paste this when reporting the problem.",
  });
}
</script>

<template>
  <DialogFrame :open="open" title="Logs" width="xl" @update:open="emit('update:open', $event)">
    <template #header-extra>
      <SegmentedControl
        :model-value="activeTab"
        :options="tabOptions"
        size="md"
        variant="plain"
        aria-label="Log source"
        @update:model-value="onTab"
      />
    </template>

    <pre
      data-testid="gui-log"
      class="h-[min(32rem,65vh)] overflow-auto whitespace-pre-wrap break-all rounded-lg border bg-sidebar p-3.5 font-mono text-[11px] leading-[20px] text-muted-foreground"
    ><template v-if="activeLines.length">{{ activeLines.join("\n") }}</template><span v-else class="text-faint">No log entries yet.</span></pre>

    <template #footer-start>
      <span
        v-if="activePath"
        class="truncate font-mono"
        :title="activePath"
        data-testid="gui-log-path"
      >{{ activePath }}</span>
    </template>
    <template #footer>
      <Button variant="outline" @click="fetchLogs()"><RefreshCw /> Refresh</Button>
      <Button variant="outline" :disabled="!activeLines.length" @click="copyActive">
        <Copy /> Copy
      </Button>
      <Button variant="outline" @click="emit('update:open', false)">Close</Button>
    </template>
  </DialogFrame>
</template>
