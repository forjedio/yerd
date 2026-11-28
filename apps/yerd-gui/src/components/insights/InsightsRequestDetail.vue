<script setup lang="ts">
import { ChevronDown, ChevronRight, TriangleAlert, X } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Spinner from "@/components/ui/Spinner.vue";
import { openInEditor } from "@/ipc/client";
import type { InsightsRequestDetail, InsightsRequestIssue } from "@/ipc/types";
import { TAG } from "@/lib/dumps";
import {
  breakdownBar,
  durationTone,
  filterTimeline,
  formatMs,
  groupTimeline,
  methodTone,
  requestIssueDetail,
  requestIssueTitle,
  requestMeta,
  requestParts,
  statusTone,
  timelineChips,
  type TimelineChipKey,
  type TimelineEventRow,
  type TimelineRow,
} from "@/lib/insights";
import { toastOnError } from "@/lib/toastActions";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * One request's drill-in: the header (method, path, status, time, route), the
 * time breakdown bar, a callout per detected issue with "Open in editor", and
 * the event timeline with category chips. N+1 runs collapse into one row that
 * expands to each query; the last row is the response itself.
 */
const props = defineProps<{
  detail: InsightsRequestDetail | null;
  loading?: boolean;
  error?: string | null;
  /** Show a close button (drawer mode). */
  closable?: boolean;
}>();

const emit = defineEmits<{ close: [] }>();

const chip = ref<TimelineChipKey>("all");
const expanded = ref(new Set<string>());

watch(
  () => props.detail?.summary.id,
  () => {
    chip.value = "all";
    expanded.value = new Set();
  },
);

const summary = computed(() => props.detail?.summary ?? null);
const segments = computed(() => (summary.value ? breakdownBar(requestParts(summary.value)) : []));
const chips = computed(() => timelineChips(props.detail?.events ?? []));
const rows = computed(() => (props.detail ? filterTimeline(groupTimeline(props.detail), chip.value) : []));
const totalMs = computed(() => {
  const s = summary.value;
  if (!s) return 1;
  const ends = (props.detail?.events ?? []).map((e) => ((e.t_us ?? 0) + (e.dur_us ?? 0)) / 1000);
  return Math.max(s.duration_ms, ...ends, 1);
});
const issues = computed(() => [...(props.detail?.issues ?? [])].sort((a, b) => b.time_ms - a.time_ms));

function toggle(key: string): void {
  const next = new Set(expanded.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expanded.value = next;
}

function segmentText(part: string, ms: number): string {
  const s = summary.value;
  if (part === "db" && s) return `${formatMs(ms)} · ${s.queries} ${s.queries === 1 ? "query" : "queries"}`;
  return formatMs(ms);
}

function bar(offset: number | null, dur: number | null): { left: string; width: string } | null {
  if (offset == null) return null;
  const left = Math.min(1, Math.max(0, offset / totalMs.value));
  const width = Math.max(0.004, Math.min(1 - left, (dur ?? 0) / totalMs.value));
  return { left: `${(left * 100).toFixed(2)}%`, width: `${(width * 100).toFixed(2)}%` };
}

function rowBar(r: TimelineRow): { left: string; width: string } | null {
  if (r.type === "done") return { left: "0%", width: "100%" };
  if (r.type === "group") return bar(r.offsetMs, r.spanMs);
  return bar(r.offsetMs, r.durMs);
}

function offsetLabel(ms: number | null): string {
  return ms == null ? "" : `+${ms.toFixed(1)}`;
}

function rowTime(r: TimelineRow): string {
  if (r.type === "done") return r.offsetMs.toFixed(0);
  if (r.type === "group") return r.totalMs >= 10 ? r.totalMs.toFixed(0) : r.totalMs.toFixed(1);
  return r.durMs == null ? "·" : r.durMs.toFixed(1);
}

const TAG_TONE: Record<string, string> = {
  QUERY: CHIP_TONE.warning,
  DUMP: CHIP_TONE.brand,
  HTTP: "bg-violet-400/15 text-violet-400",
  DONE: CHIP_TONE.success,
};

function tagClass(r: TimelineRow): string {
  if (r.type === "group") return "bg-warning text-warning-foreground";
  return TAG_TONE[r.tag] ?? CHIP_TONE.neutral;
}

function barClass(r: TimelineRow | TimelineEventRow): string {
  if (r.type === "done") return "bg-muted-foreground/30";
  if (r.type === "group") return "bg-warning";
  switch (r.event.category) {
    case "query":
      return "bg-warning";
    case "http":
      return "bg-violet-400";
    case "dump":
      return "bg-brand";
    default:
      return "bg-muted-foreground";
  }
}

async function openIssue(i: InsightsRequestIssue): Promise<void> {
  const file = i.file;
  if (!file) return;
  await toastOnError("Couldn't open the editor", () => openInEditor(file));
}

const ROW = "grid grid-cols-[52px_84px_minmax(0,1.6fr)_minmax(48px,0.6fr)_44px] items-center gap-x-3 px-4";
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col" data-testid="insights-detail">
    <div v-if="loading && !detail" class="flex flex-1 items-center justify-center py-16">
      <Spinner />
    </div>
    <div v-else-if="error && !detail" class="flex flex-1 items-center justify-center px-6 py-16 text-center">
      <p class="text-[13px] text-muted-foreground">{{ error }}</p>
    </div>
    <div v-else-if="!summary" class="flex flex-1 items-center justify-center px-6 py-16 text-center">
      <p class="text-[13px] text-muted-foreground">Pick a request to see where its time went.</p>
    </div>

    <div v-else class="flex flex-col gap-4 px-7 py-6">
      <header class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <h3 class="flex min-w-0 items-center gap-2.5">
            <span :class="cn(TAG, CHIP_TONE[methodTone(summary.method)], 'h-6 px-2 text-xs')">{{ summary.method }}</span>
            <span class="truncate font-mono text-xl font-semibold tracking-[-0.3px]" :title="summary.path">
              {{ summary.path }}
            </span>
          </h3>
          <p class="mt-2 truncate font-mono text-xs text-muted-foreground">{{ requestMeta(summary) }}</p>
        </div>
        <span :class="cn(TAG, CHIP_TONE[statusTone(summary.status)], 'h-7 px-2.5 text-[13px]')" data-testid="detail-status">
          {{ summary.status }}
        </span>
        <span
          :class="cn(TAG, CHIP_TONE[summary.cold ? 'brand' : durationTone(summary.duration_ms)], 'h-7 px-2.5 text-[13px]')"
          data-testid="detail-time"
        >
          {{ formatMs(summary.duration_ms) }}
        </span>
        <button
          v-if="closable"
          type="button"
          class="flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-label="Close"
          @click="emit('close')"
        >
          <X class="size-4" />
        </button>
      </header>

      <p v-if="summary.cold" class="rounded-md bg-brand/10 px-3 py-2 text-[13px] text-brand">
        Cold start: the first request a freshly spawned PHP worker served. Not counted in the statistics.
      </p>

      <div v-if="segments.length" data-testid="detail-breakdown">
        <div class="flex h-1.5 gap-0.5 overflow-hidden rounded-full" aria-hidden="true">
          <span
            v-for="s in segments"
            :key="s.part"
            :class="cn('h-full first:rounded-l-full last:rounded-r-full', s.fill)"
            :style="{ width: `${(s.share * 100).toFixed(2)}%` }"
          />
        </div>
        <p class="mt-2.5 flex flex-wrap gap-x-6 gap-y-1 text-[13px] text-muted-foreground">
          <span v-for="s in segments" :key="s.part" :class="s.part === 'db' && s.share >= 0.4 ? 'text-warning' : ''">
            {{ s.short }} {{ segmentText(s.part, s.ms) }}
          </span>
        </p>
      </div>
      <p v-else-if="!summary.traced" class="text-[13px] text-faint">
        Timing only: the Yerd PHP extension did not trace this request.
      </p>

      <div
        v-for="i in issues"
        :key="i.key"
        class="flex items-start gap-3 rounded-lg border border-warning/40 bg-warning/10 px-4 py-3"
        data-testid="issue-callout"
      >
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-warning" aria-hidden="true" />
        <div class="min-w-0 flex-1">
          <p class="text-sm font-semibold">{{ requestIssueTitle(i) }}</p>
          <p class="mt-0.5 text-[13px] text-muted-foreground">{{ requestIssueDetail(i) }}</p>
        </div>
        <button
          v-if="i.file"
          type="button"
          class="shrink-0 text-[13px] font-semibold text-warning hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          data-testid="open-in-editor"
          @click="openIssue(i)"
        >
          Open in editor
        </button>
      </div>

      <section v-if="detail && detail.events.length" class="flex flex-col gap-3" data-testid="timeline">
        <header class="flex flex-wrap items-center gap-2">
          <h4 class="font-mono text-[11px] uppercase tracking-[1.4px] text-faint">
            Timeline · {{ chips[0]?.count ?? 0 }} events
          </h4>
          <div class="ml-auto flex flex-wrap items-center gap-1" role="group" aria-label="Filter events">
            <button
              v-for="c in chips"
              :key="c.key"
              type="button"
              :aria-pressed="chip === c.key"
              :data-chip="c.key"
              :class="
                cn(
                  'h-7 rounded-md px-2.5 text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                  chip === c.key ? 'bg-accent font-semibold text-foreground' : 'text-muted-foreground hover:text-foreground',
                )
              "
              @click="chip = c.key"
            >
              {{ c.label }}<span v-if="c.key !== 'all'" class="ml-1">{{ c.count }}</span>
            </button>
          </div>
        </header>

        <div class="overflow-hidden rounded-lg border bg-card">
          <template v-for="r in rows" :key="r.key">
            <component
              :is="r.type === 'group' ? 'button' : 'div'"
              :type="r.type === 'group' ? 'button' : undefined"
              :aria-expanded="r.type === 'group' ? expanded.has(r.key) : undefined"
              :class="
                cn(
                  ROW,
                  'border-b py-3 last:border-b-0',
                  r.type === 'group' &&
                    'w-full cursor-pointer bg-warning/[0.08] text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
                )
              "
              :data-row="r.type"
              @click="r.type === 'group' && toggle(r.key)"
            >
              <span class="font-mono text-xs text-faint">{{ offsetLabel(r.offsetMs) }}</span>
              <span :class="cn(TAG, tagClass(r), 'h-6 w-full justify-center')">{{ r.tag }}</span>
              <span class="min-w-0">
                <span class="flex min-w-0 items-center gap-1.5">
                  <component
                    :is="expanded.has(r.key) ? ChevronDown : ChevronRight"
                    v-if="r.type === 'group'"
                    class="size-3.5 shrink-0 text-warning"
                    aria-hidden="true"
                  />
                  <span class="truncate font-mono text-[13px]" :title="r.text">{{ r.text }}</span>
                </span>
                <span v-if="r.type === 'group'" class="mt-0.5 block text-xs text-muted-foreground">
                  {{ expanded.has(r.key) ? "Each binding below." : "Grouped. Expand to see each binding." }}
                </span>
              </span>
              <span class="relative h-1.5" aria-hidden="true">
                <span
                  v-if="rowBar(r)"
                  :class="cn('absolute top-0 h-full rounded-full', barClass(r))"
                  :style="rowBar(r) ?? undefined"
                />
              </span>
              <span :class="cn('text-right font-mono text-xs', r.type === 'group' ? 'text-warning' : 'text-muted-foreground')">
                {{ rowTime(r) }}
              </span>
            </component>
            <template v-if="r.type === 'group' && expanded.has(r.key)">
              <div
                v-for="c in r.children"
                :key="c.key"
                :class="cn(ROW, 'border-b bg-warning/[0.04] py-2')"
                data-row="child"
              >
                <span class="font-mono text-xs text-faint">{{ offsetLabel(c.offsetMs) }}</span>
                <span />
                <span class="truncate font-mono text-xs text-muted-foreground" :title="c.text">{{ c.text }}</span>
                <span class="relative h-1" aria-hidden="true">
                  <span
                    v-if="bar(c.offsetMs, c.durMs)"
                    class="absolute top-0 h-full rounded-full bg-warning/70"
                    :style="bar(c.offsetMs, c.durMs) ?? undefined"
                  />
                </span>
                <span class="text-right font-mono text-xs text-muted-foreground">
                  {{ c.durMs == null ? "·" : c.durMs.toFixed(1) }}
                </span>
              </div>
            </template>
          </template>
        </div>
        <p v-if="detail.truncated" class="text-xs text-faint">
          The timeline was cut short: this request hit the event cap.
        </p>
      </section>
    </div>
  </div>
</template>
