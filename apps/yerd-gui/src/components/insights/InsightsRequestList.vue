<script setup lang="ts">
import { Search } from "lucide-vue-next";
import { onBeforeUnmount, ref, watch } from "vue";

import Select from "@/components/ui/Select.vue";
import type { InsightsRequestSummary, InsightsSort } from "@/ipc/types";
import { TAG } from "@/lib/dumps";
import {
  durationTone,
  formatClock,
  formatMs,
  methodTone,
  requestSubline,
  statusTone,
  TONE_TEXT,
} from "@/lib/insights";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * The recorded requests with a filter box and "Slow first" / "Recent" order.
 * `list` is the column beside the drill-in (wide windows); `table` spans the
 * page (compact) with one column per figure. Cold requests are labelled "cold
 * start, not in stats" and their time is greyed, since statistics skip them.
 * The filter text reaches the daemon 250ms after typing stops.
 */
defineProps<{
  layout: "list" | "table";
  requests: readonly InsightsRequestSummary[];
  selected: string | null;
  loaded: boolean;
}>();

const query = defineModel<string>("query", { required: true });
const sort = defineModel<InsightsSort>("sort", { required: true });

const emit = defineEmits<{ select: [request: InsightsRequestSummary] }>();

const SORTS: readonly { value: InsightsSort; label: string }[] = [
  { value: "slowest", label: "Slow first" },
  { value: "recent", label: "Recent" },
];

const draft = ref(query.value);
let timer: ReturnType<typeof setTimeout> | null = null;

watch(draft, (v) => {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    query.value = v;
  }, 250);
});

watch(query, (v) => {
  if (v !== draft.value) draft.value = v;
});

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer);
});

function timeClass(r: InsightsRequestSummary): string {
  if (r.cold) return "text-faint";
  if (r.status >= 500) return "text-destructive";
  const tone = durationTone(r.duration_ms);
  return tone === "neutral" ? "text-foreground" : TONE_TEXT[tone];
}

const TABLE_COLS = "grid grid-cols-[64px_minmax(0,1fr)_64px_84px_72px_72px_84px] items-center gap-x-3";

</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="insights-requests" :data-layout="layout">
    <div class="flex shrink-0 items-center gap-2 border-b px-5 py-2.5">
      <label class="relative min-w-0 flex-1">
        <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint" aria-hidden="true" />
        <input
          v-model="draft"
          type="search"
          placeholder="Filter path, status, route…"
          aria-label="Filter requests"
          class="h-8 w-full rounded-[7px] border bg-muted pl-8 pr-2.5 text-[13px] placeholder:text-faint focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          data-testid="request-filter"
        />
      </label>
      <Select v-model="sort" :options="SORTS" aria-label="Order" class="w-[120px] bg-muted text-[13px]" />
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto">
      <p v-if="!loaded" class="px-5 py-10 text-center text-[13px] text-faint">Loading…</p>
      <p v-else-if="requests.length === 0" class="px-5 py-10 text-center text-[13px] text-muted-foreground" data-testid="requests-empty">
        {{ query.trim() ? `No requests match “${query.trim()}”` : "No requests in this range yet." }}
      </p>

      <template v-else-if="layout === 'list'">
        <button
          v-for="r in requests"
          :key="r.id"
          type="button"
          :data-id="r.id"
          :aria-current="selected === r.id ? 'true' : undefined"
          :class="
            cn(
              'grid w-full grid-cols-[52px_minmax(0,1fr)_44px_76px] items-center gap-x-3 border-b px-5 py-3 text-left transition-colors focus-visible:outline-none',
              selected === r.id ? 'bg-selected' : 'hover:bg-accent/60 focus-visible:bg-accent',
            )
          "
          @click="emit('select', r)"
        >
          <span :class="cn(TAG, CHIP_TONE[methodTone(r.method)], 'justify-center')">{{ r.method }}</span>
          <span class="min-w-0">
            <span class="block truncate font-mono text-[13px] font-medium">{{ r.path }}</span>
            <span :class="cn('mt-0.5 block truncate text-xs', r.cold ? 'text-brand' : 'text-faint')" data-testid="request-sub">
              {{ requestSubline(r) }}
            </span>
          </span>
          <span :class="cn('text-right font-mono text-[13px]', TONE_TEXT[statusTone(r.status)])">{{ r.status }}</span>
          <span :class="cn('text-right font-mono text-[13px]', timeClass(r))">{{ formatMs(r.duration_ms) }}</span>
        </button>
      </template>

      <template v-else>
        <div
          :class="cn(TABLE_COLS, 'px-5 pb-1.5 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
          aria-hidden="true"
        >
          <span>Method</span><span>Path</span><span class="text-right">Status</span><span class="text-right">Time</span>
          <span class="text-right">Queries</span><span class="text-right">Dumps</span><span class="text-right">At</span>
        </div>
        <button
          v-for="r in requests"
          :key="r.id"
          type="button"
          :data-id="r.id"
          :class="cn(TABLE_COLS, 'w-full border-t px-5 py-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/20 focus-visible:outline-none')"
          @click="emit('select', r)"
        >
          <span :class="cn(TAG, CHIP_TONE[methodTone(r.method)], 'w-fit')">{{ r.method }}</span>
          <span class="min-w-0">
            <span class="block truncate font-mono text-[13px]">{{ r.path }}</span>
            <span v-if="r.cold" class="block truncate text-xs text-brand" data-testid="request-sub">cold start, not in stats</span>
          </span>
          <span :class="cn('text-right font-mono text-[13px]', TONE_TEXT[statusTone(r.status)])">{{ r.status }}</span>
          <span :class="cn('text-right font-mono text-[13px]', timeClass(r))">{{ formatMs(r.duration_ms) }}</span>
          <span class="text-right font-mono text-[13px] text-muted-foreground">{{ r.queries }}</span>
          <span class="text-right font-mono text-[13px] text-muted-foreground">{{ r.dumps }}</span>
          <span class="text-right font-mono text-xs text-faint">{{ formatClock(r.ts_ms) }}</span>
        </button>
      </template>
    </div>
  </div>
</template>
