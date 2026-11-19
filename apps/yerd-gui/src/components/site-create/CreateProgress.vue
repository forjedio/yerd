<script setup lang="ts">
import { Check, CheckCircle2, ChevronDown, ChevronRight, Circle, Copy, Loader2, Lock, LockOpen } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import { phaseStatus } from "@/composables/useCreateSiteJob";
import { useToast } from "@/composables/useToast";
import type { JobState } from "@/ipc/types";
import { formatDuration } from "@/lib/createSite";
import { cn } from "@/lib/utils";

/**
 * The New site dialog's job view: one row per daemon phase (the active one shows
 * the latest log line), the full log on demand, and the failure or cancelled
 * note. Once the job succeeds it becomes the Done screen.
 */
const props = defineProps<{
  phases: string[];
  phase: string;
  state: JobState;
  log: string[];
  error: string | null;
  domain: string;
  openUrl: string;
  secure: boolean;
  doneSummary: string;
  startedAt: number | null;
  finishedAt: number | null;
}>();

const toast = useToast();
const showLog = ref(true);
const logBox = ref<HTMLElement | null>(null);

watch(
  () => props.log.length,
  () => {
    if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight;
  },
  { flush: "post" },
);

const latestLine = computed(() => props.log[props.log.length - 1] ?? "");

/** Daemon phase names, worded for the list as drawn. */
function label(p: string): string {
  if (p === "Registering") return "Registering with Yerd";
  if (p === "Done") return `Ready at ${props.openUrl}`;
  return p;
}

const took = computed(() =>
  props.startedAt !== null && props.finishedAt !== null
    ? ` · took ${formatDuration(props.finishedAt - props.startedAt)}`
    : "",
);

async function copyUrl(): Promise<void> {
  try {
    await navigator.clipboard.writeText(props.openUrl);
    toast.success("Copied URL");
  } catch {
    toast.error("Couldn't copy URL", "Your browser blocked clipboard access.");
  }
}
</script>

<template>
  <div v-if="state === 'succeeded'" class="flex flex-col items-center py-4 text-center" data-testid="ns-done">
    <span
      class="flex size-12 items-center justify-center rounded-full border border-success/40 bg-success/10 text-success"
    >
      <Check class="size-5" aria-hidden="true" />
    </span>
    <p class="mt-4 text-[17px] font-semibold">{{ domain }} is ready</p>
    <div class="mt-3 flex h-8 max-w-full items-center gap-2 rounded-md border bg-background px-3">
      <Lock v-if="secure" class="size-3 shrink-0 text-success" aria-hidden="true" />
      <LockOpen v-else class="size-3 shrink-0 text-faint" aria-hidden="true" />
      <span class="truncate font-mono text-[13px]">{{ openUrl }}</span>
      <button
        type="button"
        class="shrink-0 text-faint hover:text-foreground"
        aria-label="Copy URL"
        @click="copyUrl"
      >
        <Copy class="size-3.5" />
      </button>
    </div>
    <p class="mt-3 text-[13px] text-muted-foreground" data-testid="ns-summary">
      {{ doneSummary }}{{ took }}
    </p>
  </div>

  <div v-else class="space-y-3">
    <div class="divide-y overflow-hidden rounded-[10px] border">
      <div
        v-for="p in phases"
        :key="p"
        :class="
          cn(
            'flex min-h-11 items-center gap-3 px-3.5 py-2.5',
            phaseStatus(phases, phase, state, p) === 'active' && 'bg-brand/10',
          )
        "
        :data-phase="p"
      >
        <CheckCircle2
          v-if="phaseStatus(phases, phase, state, p) === 'done'"
          class="size-4 shrink-0 text-success"
          aria-label="Done"
        />
        <Loader2
          v-else-if="phaseStatus(phases, phase, state, p) === 'active'"
          class="size-4 shrink-0 animate-spin text-brand"
          aria-label="Running"
        />
        <Circle v-else class="size-4 shrink-0 text-faint" aria-hidden="true" />
        <div class="min-w-0">
          <p
            :class="
              cn(
                'text-[13px]',
                phaseStatus(phases, phase, state, p) === 'todo' ? 'text-faint' : 'font-semibold',
              )
            "
          >
            {{ label(p) }}
          </p>
          <p
            v-if="phaseStatus(phases, phase, state, p) === 'active' && latestLine"
            class="truncate font-mono text-[11px] text-muted-foreground"
          >
            {{ latestLine }}
          </p>
        </div>
      </div>
    </div>
    <p v-if="phase && !phases.includes(phase) && state === 'running'" class="text-xs text-muted-foreground">
      {{ phase }}…
    </p>

    <button
      type="button"
      class="flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
      @click="showLog = !showLog"
    >
      <component :is="showLog ? ChevronDown : ChevronRight" class="size-3.5" />
      {{ showLog ? "Hide log" : "Show log" }}
    </button>
    <pre
      v-if="showLog"
      ref="logBox"
      class="h-40 overflow-y-auto whitespace-pre-wrap rounded-[10px] bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
    >{{ log.join("\n") || "Starting…" }}</pre>

    <div
      v-if="state === 'failed'"
      class="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
    >
      {{ error || "Creation failed." }}
    </div>
    <div v-else-if="state === 'cancelled'" class="rounded-lg border p-3 text-sm text-muted-foreground">
      Cancelled.
    </div>
  </div>
</template>
