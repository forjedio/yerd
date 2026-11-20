<script setup lang="ts">
import { nextTick, onUnmounted, ref, watch } from "vue";
import { Copy } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, serviceLogs } from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { copyWithToast } from "@/lib/toastActions";
import { logLineTone } from "@/lib/services";
import { cn } from "@/lib/utils";

/**
 * A service's log tail. The daemon only tails the file on request, so "live"
 * means polling the last 500 lines every 2s while the dialog is open, one
 * request at a time. A failed read toasts once per open, not on every tick. The
 * box follows new lines only while it is scrolled to the bottom, so reading
 * older lines isn't interrupted.
 */
const props = defineProps<{
  open: boolean;
  service: ServiceStatus | null;
  title: string;
}>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const LINES = 500;
const POLL_MS = 2000;

const toast = useToast();
const lines = ref<string[]>([]);
const live = ref(false);
const box = ref<HTMLElement | null>(null);

let timer: ReturnType<typeof setInterval> | null = null;
let generation = 0;
let inFlight = false;
let warned = false;

function nearBottom(): boolean {
  const el = box.value;
  return !el || el.scrollHeight - el.scrollTop - el.clientHeight <= 24;
}

async function fetchLogs(): Promise<void> {
  const s = props.service;
  if (!s || inFlight) return;
  const gen = generation;
  const follow = nearBottom();
  inFlight = true;
  try {
    const next = await serviceLogs(s.service, LINES);
    if (gen !== generation) return;
    lines.value = next;
    live.value = true;
    if (follow) {
      await nextTick();
      if (box.value) box.value.scrollTop = box.value.scrollHeight;
    }
  } catch (e) {
    if (gen !== generation) return;
    live.value = false;
    if (!warned) {
      warned = true;
      toast.error("Couldn't read logs", (e as IpcError).message);
    }
  } finally {
    if (gen === generation) inFlight = false;
  }
}

function stop(): void {
  if (timer !== null) clearInterval(timer);
  timer = null;
  generation += 1;
  inFlight = false;
}

function start(): void {
  stop();
  lines.value = [];
  live.value = false;
  warned = false;
  void fetchLogs();
  timer = setInterval(() => void fetchLogs(), POLL_MS);
}

watch(
  [() => props.open, () => props.service?.service],
  ([open, id]) => {
    if (open && id) start();
    else stop();
  },
  { immediate: true },
);

onUnmounted(stop);

async function copy(): Promise<void> {
  await copyWithToast(lines.value.join("\n"), {
    title: "Copied to clipboard",
    detail: `${lines.value.length} log lines`,
  });
}
</script>

<template>
  <DialogFrame :open="open" :title="title" width="xl" @update:open="emit('update:open', $event)">
    <template #title>
      <span class="truncate">{{ title }}</span>
      <span
        v-if="live"
        class="inline-flex h-5 shrink-0 items-center gap-1.5 rounded bg-success/15 px-2 text-[11px] font-medium text-success"
        data-testid="logs-live"
      >
        <span class="size-1.5 rounded-full bg-success" aria-hidden="true" />
        Live
      </span>
    </template>

    <pre
      ref="box"
      data-testid="service-log"
      class="h-[min(24rem,50vh)] overflow-y-auto whitespace-pre-wrap break-all rounded-lg border bg-sidebar p-3.5 font-mono text-[11px] leading-[20px] text-muted-foreground"
    ><template v-if="lines.length"><span
      v-for="(line, i) in lines"
      :key="i"
      :class="cn('block', logLineTone(line) === 'error' ? 'text-destructive' : i === lines.length - 1 && 'text-foreground')"
    >{{ line || " " }}</span></template><span v-else class="text-faint">No log output yet.</span></pre>

    <template #footer-start>Last {{ LINES }} lines</template>
    <template #footer>
      <Button variant="outline" :disabled="!lines.length" @click="copy"><Copy /> Copy</Button>
      <Button variant="outline" @click="emit('update:open', false)">Close</Button>
    </template>
  </DialogFrame>
</template>
