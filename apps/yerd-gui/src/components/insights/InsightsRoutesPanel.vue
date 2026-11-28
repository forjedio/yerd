<script setup lang="ts">
import { computed } from "vue";

import { CARD } from "@/components/ui/styles";
import type { RoutesTab } from "@/composables/useInsights";
import type { InsightsQuery, InsightsRequestSummary, InsightsRoute } from "@/ipc/types";
import { TAG } from "@/lib/dumps";
import {
  durationTone,
  formatClock,
  formatCount,
  formatMs,
  ISSUE_LABEL,
  methodTone,
  statusTone,
  TONE_TEXT,
} from "@/lib/insights";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * The Overview's bottom panel: Routes (by total time), the slowest ten
 * Requests, and Queries (fingerprints by total time). A request row opens its
 * drill-in.
 */
defineProps<{
  routes: readonly InsightsRoute[];
  queries: readonly InsightsQuery[];
  slowest: readonly InsightsRequestSummary[];
}>();

const tab = defineModel<RoutesTab>("tab", { required: true });

const emit = defineEmits<{ open: [request: InsightsRequestSummary] }>();

const TABS: readonly { key: RoutesTab; label: string }[] = [
  { key: "routes", label: "Routes" },
  { key: "requests", label: "Requests" },
  { key: "queries", label: "Queries" },
];

const caption = computed(() =>
  tab.value === "requests" ? "Slowest 10 in this range" : "Sorted by total time spent",
);

const HEAD = "px-5 pb-1.5 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint";
const ROUTE_COLS = "grid grid-cols-[minmax(0,1fr)_80px_72px_72px_120px_80px_minmax(0,150px)] items-center gap-x-3";
const REQ_COLS = "grid grid-cols-[minmax(0,1fr)_64px_80px_80px_90px] items-center gap-x-3";
const QUERY_COLS = "grid grid-cols-[minmax(0,1fr)_70px_80px_80px_minmax(0,200px)] items-center gap-x-3";
</script>

<template>
  <section :class="cn(CARD, 'flex min-w-0 flex-col')" data-testid="routes-panel">
    <header class="flex h-12 items-center gap-5 border-b px-5">
      <button
        v-for="t in TABS"
        :key="t.key"
        type="button"
        :class="
          cn(
            '-mb-px flex h-12 items-center border-b-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            tab === t.key ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground',
          )
        "
        :aria-pressed="tab === t.key"
        :data-tab="t.key"
        @click="tab = t.key"
      >
        {{ t.label }}
      </button>
      <span class="ml-auto text-xs text-faint">{{ caption }}</span>
    </header>

    <template v-if="tab === 'routes'">
      <p v-if="routes.length === 0" class="px-5 py-8 text-center text-[13px] text-muted-foreground">No routes yet.</p>
      <template v-else>
        <div :class="cn(ROUTE_COLS, HEAD)" aria-hidden="true">
          <span>Route</span><span class="text-right">Requests</span><span class="text-right">p50</span>
          <span class="text-right">p95</span><span>Time share</span><span class="text-right">Queries</span><span>Issues</span>
        </div>
        <div
          v-for="r in routes"
          :key="`${r.method} ${r.label}`"
          :class="cn(ROUTE_COLS, 'border-t px-5 py-2.5')"
          data-testid="route-row"
        >
          <span class="flex min-w-0 items-center gap-2">
            <span :class="cn(TAG, CHIP_TONE[methodTone(r.method)])">{{ r.method }}</span>
            <span class="min-w-0">
              <span class="block truncate font-mono text-[13px]">{{ r.label }}</span>
              <span v-if="r.name || r.action" class="block truncate text-xs text-faint">
                {{ [r.name, r.action].filter(Boolean).join(" · ") }}
              </span>
            </span>
          </span>
          <span class="text-right font-mono text-[13px]">{{ formatCount(r.requests) }}</span>
          <span class="text-right font-mono text-[13px]">{{ formatMs(r.p50_ms) }}</span>
          <span :class="cn('text-right font-mono text-[13px]', r.p95_ms != null && TONE_TEXT[durationTone(r.p95_ms)])">
            {{ formatMs(r.p95_ms) }}
          </span>
          <span class="flex items-center gap-2">
            <span class="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
              <span class="block h-full rounded-full bg-brand" :style="{ width: `${Math.round(r.share * 100)}%` }" />
            </span>
            <span class="w-9 text-right font-mono text-xs text-muted-foreground">{{ Math.round(r.share * 100) }}%</span>
          </span>
          <span class="text-right font-mono text-[13px]">
            {{ r.queries_per_request == null ? "-" : r.queries_per_request.toFixed(1).replace(/\.0$/, "") }}
          </span>
          <span class="flex min-w-0 flex-wrap gap-1">
            <span v-for="k in r.issue_kinds ?? []" :key="k" :class="cn(TAG, CHIP_TONE.warning)">{{ ISSUE_LABEL[k] }}</span>
          </span>
        </div>
      </template>
    </template>

    <template v-else-if="tab === 'requests'">
      <p v-if="slowest.length === 0" class="px-5 py-8 text-center text-[13px] text-muted-foreground">No requests yet.</p>
      <template v-else>
        <div :class="cn(REQ_COLS, HEAD)" aria-hidden="true">
          <span>Request</span><span class="text-right">Status</span><span class="text-right">Time</span>
          <span class="text-right">Queries</span><span class="text-right">At</span>
        </div>
        <button
          v-for="r in slowest"
          :key="r.id"
          type="button"
          :class="cn(REQ_COLS, 'w-full border-t px-5 py-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/20 focus-visible:outline-none')"
          data-testid="slow-row"
          @click="emit('open', r)"
        >
          <span class="flex min-w-0 items-center gap-2">
            <span :class="cn(TAG, CHIP_TONE[methodTone(r.method)])">{{ r.method }}</span>
            <span class="truncate font-mono text-[13px]">{{ r.path }}</span>
          </span>
          <span :class="cn('text-right font-mono text-[13px]', TONE_TEXT[statusTone(r.status)])">{{ r.status }}</span>
          <span :class="cn('text-right font-mono text-[13px]', TONE_TEXT[durationTone(r.duration_ms)])">
            {{ formatMs(r.duration_ms) }}
          </span>
          <span class="text-right font-mono text-[13px] text-muted-foreground">{{ r.queries }}</span>
          <span class="text-right font-mono text-xs text-faint">{{ formatClock(r.ts_ms) }}</span>
        </button>
      </template>
    </template>

    <template v-else>
      <p v-if="queries.length === 0" class="px-5 py-8 text-center text-[13px] text-muted-foreground">No queries yet.</p>
      <template v-else>
        <div :class="cn(QUERY_COLS, HEAD)" aria-hidden="true">
          <span>Query</span><span class="text-right">Runs</span><span class="text-right">Total</span>
          <span class="text-right">Mean</span><span>Last route</span>
        </div>
        <div v-for="q in queries" :key="q.fingerprint" :class="cn(QUERY_COLS, 'border-t px-5 py-2.5')" data-testid="query-row">
          <span class="truncate font-mono text-[13px]" :title="q.fingerprint">{{ q.fingerprint }}</span>
          <span class="text-right font-mono text-[13px]">{{ formatCount(q.count) }}</span>
          <span class="text-right font-mono text-[13px]">{{ formatMs(q.total_ms) }}</span>
          <span class="text-right font-mono text-[13px] text-muted-foreground">{{ formatMs(q.mean_ms) }}</span>
          <span class="truncate font-mono text-xs text-muted-foreground">{{ q.route_label ?? "" }}</span>
        </div>
      </template>
    </template>
  </section>
</template>
