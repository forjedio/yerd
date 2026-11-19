<script setup lang="ts">
import { Antenna, Layers, Play, Radio, Trash2 } from "lucide-vue-next";
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { DumpsBusy } from "@/composables/useDumps";
import type { DumpsStatusResponse } from "@/ipc/types";
import {
  type ChipKey,
  chipLabel,
  type DumpRequest,
  eventsLabel,
  formatClock,
  listDuration,
  requestTitle,
  statusTone,
  TAG,
  TAG_TONE,
} from "@/lib/dumps";
import { cn } from "@/lib/utils";

/**
 * The captured requests: a "Requests" header with Keep history and Clear, then
 * the rows. `list` is the column beside the detail (wide windows and the Dumps
 * window); `table` spans the window (compact) with METHOD to AT columns, where
 * a click opens the detail drawer and no row stays highlighted. With nothing
 * captured it shows the paused state (with Resume) or the waiting state.
 */
const props = defineProps<{
  layout: "list" | "table";
  /** The requests the filter keeps. */
  requests: DumpRequest[];
  /** Every held request, whatever the filter. */
  total: number;
  selected: string | null;
  status: DumpsStatusResponse | null;
  busy: DumpsBusy;
  query: string;
  category: ChipKey;
}>();

const emit = defineEmits<{
  select: [key: string];
  togglePersist: [next: boolean];
  clear: [];
  resume: [];
  clearFilter: [];
}>();

const persist = computed(() => props.status?.persist ?? false);
const paused = computed(() => !!props.status && !props.status.enabled);
const noMatch = computed(() =>
  props.query.trim() ? `No requests match “${props.query.trim()}”` : `No requests with ${chipLabel(props.category).toLowerCase()}`,
);

function sub(r: DumpRequest): string {
  if (!r.summary) return `No request summary · ${eventsLabel(r.events.length)}`;
  const parts = [r.site, eventsLabel(r.events.length)];
  if (r.summary.durationMs != null) parts.push(listDuration(r.summary.durationMs));
  return parts.filter(Boolean).join(" · ");
}

const TABLE_COLS = "grid grid-cols-[70px_64px_minmax(0,1fr)_140px_70px_80px_80px] items-center gap-x-2";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="dumps-requests" :data-layout="layout">
    <div class="flex h-[46px] shrink-0 items-center gap-2 pr-4">
      <h3 class="text-[15px] font-semibold">Requests</h3>
      <span
        class="inline-flex h-5 min-w-[25px] items-center justify-center rounded-full bg-muted px-2 text-[11px] font-semibold text-muted-foreground"
        data-testid="requests-count"
      >
        {{ total }}
      </span>
      <div class="ml-auto flex items-center gap-2">
        <button
          type="button"
          :class="
            cn(
              'inline-flex h-[30px] items-center gap-1.5 rounded-md border px-2.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50',
              persist ? 'border-brand/45 bg-brand/[0.14] text-foreground hover:bg-brand/20' : 'bg-background text-muted-foreground hover:bg-accent hover:text-foreground',
            )
          "
          :disabled="!status || busy === 'persist'"
          :aria-pressed="persist"
          title="Keep requests when new ones arrive"
          @click="emit('togglePersist', !persist)"
        >
          <Spinner v-if="busy === 'persist'" class="size-3.5 text-current" />
          <Layers v-else :class="cn('size-3.5', persist && 'text-brand')" />
          Keep history
        </button>
        <Button
          variant="outline"
          size="icon"
          class="size-[30px]"
          :disabled="total === 0 || busy === 'clear'"
          aria-label="Clear dumps"
          title="Clear dumps"
          @click="emit('clear')"
        >
          <Spinner v-if="busy === 'clear'" class="size-3.5" />
          <Trash2 v-else class="!size-3.5" />
        </Button>
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <div
        v-if="total === 0"
        class="flex h-full flex-col items-center justify-center px-6 py-16 text-center"
        data-testid="requests-empty"
      >
        <span class="flex size-11 items-center justify-center rounded-xl border bg-card text-muted-foreground">
          <Antenna v-if="paused" class="size-5" aria-hidden="true" />
          <Radio v-else class="size-5" aria-hidden="true" />
        </span>
        <template v-if="paused">
          <p class="mt-4 text-[15px] font-semibold">Interception is paused</p>
          <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">
            Nothing is captured while paused. Resume to stream dumps again.
          </p>
          <Button size="sm" class="mt-5" :disabled="busy === 'enabled'" @click="emit('resume')">
            <Spinner v-if="busy === 'enabled'" class="text-current" />
            <Play v-else />
            Resume
          </Button>
        </template>
        <template v-else>
          <p class="mt-4 text-[15px] font-semibold">Waiting for dumps…</p>
          <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">
            Load any page on a .test site. Only web requests are captured, not artisan or queue workers.
          </p>
        </template>
      </div>

      <div
        v-else-if="requests.length === 0"
        class="flex h-full flex-col items-center justify-center px-6 py-16 text-center"
        data-testid="requests-no-match"
      >
        <p class="text-[15px] font-semibold">{{ noMatch }}</p>
        <button
          type="button"
          class="mt-1.5 text-[13px] font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="emit('clearFilter')"
        >
          Clear filter
        </button>
      </div>

      <div v-else-if="layout === 'list'" class="flex flex-col gap-0.5">
        <button
          v-for="r in requests"
          :key="r.key"
          type="button"
          :data-key="r.key"
          :aria-current="selected === r.key ? 'true' : undefined"
          :class="
            cn(
              'flex h-[62px] w-full shrink-0 flex-col justify-center gap-1 rounded-lg border px-3.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              selected === r.key ? 'border-brand/45 bg-brand/[0.14]' : 'border-transparent hover:bg-accent/60',
            )
          "
          @click="emit('select', r.key)"
        >
          <span class="flex w-full min-w-0 items-center gap-2">
            <template v-if="r.summary">
              <span v-if="r.summary.method" :class="cn(TAG, TAG_TONE.brand)">{{ r.summary.method }}</span>
              <span v-if="r.summary.status" :class="cn(TAG, TAG_TONE[statusTone(r.summary.status)])">
                {{ r.summary.status }}
              </span>
            </template>
            <span class="min-w-0 flex-1 truncate font-mono text-[13px] font-semibold">{{ requestTitle(r) }}</span>
            <span class="shrink-0 font-mono text-[11px] text-faint">{{ formatClock(r.startedMs) }}</span>
          </span>
          <span class="truncate text-xs text-muted-foreground">{{ sub(r) }}</span>
        </button>
      </div>

      <template v-else>
        <div
          :class="cn(TABLE_COLS, 'px-2.5 pb-1.5 pt-1 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
          aria-hidden="true"
        >
          <span>Method</span><span>Status</span><span>Path</span><span>Site</span><span>Events</span><span>Time</span><span>At</span>
        </div>
        <button
          v-for="r in requests"
          :key="r.key"
          type="button"
          :data-key="r.key"
          :class="
            cn(
              TABLE_COLS,
              'h-11 w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
            )
          "
          @click="emit('select', r.key)"
        >
          <span>
            <span v-if="r.summary?.method" :class="cn(TAG, TAG_TONE.brand)">{{ r.summary.method }}</span>
          </span>
          <span>
            <span v-if="r.summary?.status" :class="cn(TAG, TAG_TONE[statusTone(r.summary.status)])">
              {{ r.summary.status }}
            </span>
          </span>
          <span v-if="r.summary" class="truncate font-mono text-[13px] font-semibold">{{ r.summary.uri }}</span>
          <span v-else class="truncate text-[13px] text-muted-foreground">No request summary</span>
          <span class="truncate font-mono text-xs text-muted-foreground">{{ r.site }}</span>
          <span class="text-xs text-muted-foreground">{{ r.events.length }}</span>
          <span class="truncate font-mono text-xs text-muted-foreground">
            {{ r.summary?.durationMs != null ? listDuration(r.summary.durationMs) : "" }}
          </span>
          <span class="truncate font-mono text-[11px] text-faint">{{ formatClock(r.startedMs) }}</span>
        </button>
      </template>

      <p
        v-if="status && !persist && requests.length > 0"
        class="mt-3 px-3.5 text-xs text-faint"
        data-testid="history-footnote"
      >
        History off keeps the 25 most recent requests. On keeps everything, up to 2,000 events.
      </p>
    </div>
  </div>
</template>
