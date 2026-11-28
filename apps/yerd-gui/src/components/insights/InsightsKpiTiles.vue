<script setup lang="ts">
import { Check, Snowflake, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";

import type { InsightsOverview } from "@/ipc/types";
import { kpiTiles, statusLine, TONE_TEXT } from "@/lib/insights";
import { CARD } from "@/components/ui/styles";
import { cn } from "@/lib/utils";

/**
 * The Overview's five KPI tiles and the status line under them: cold requests
 * set aside and whether any sample was dropped.
 */
const props = defineProps<{
  overview: InsightsOverview;
  /** The "too many queries" threshold, which tints the worst-request line. */
  maxQueries?: number;
  /** Lay the tiles out in a narrower grid. */
  compact?: boolean;
}>();

const tiles = computed(() => kpiTiles(props.overview, props.maxQueries));
const line = computed(() => statusLine(props.overview));
</script>

<template>
  <div class="flex flex-col gap-3">
    <div :class="cn('grid gap-3', compact ? 'grid-cols-3' : 'grid-cols-5')" data-testid="kpi-tiles">
      <div
        v-for="t in tiles"
        :key="t.key"
        :class="cn(CARD, 'flex min-w-0 flex-col gap-1.5 px-4 py-3.5')"
        :data-kpi="t.key"
      >
        <span class="font-mono text-[10.5px] uppercase tracking-[1.2px] text-faint">{{ t.label }}</span>
        <span class="flex items-baseline gap-1">
          <span :class="cn('text-[26px] font-semibold leading-none tracking-[-0.5px]', t.valueTone !== 'neutral' && TONE_TEXT[t.valueTone])">
            {{ t.value }}
          </span>
          <span v-if="t.unit" class="text-sm text-muted-foreground">{{ t.unit }}</span>
        </span>
        <span :class="cn('truncate text-xs', TONE_TEXT[t.subTone])" :title="t.sub">
          <span v-if="t.trend === 'down'" aria-hidden="true">▼ </span>
          <span v-else-if="t.trend === 'up'" aria-hidden="true">▲ </span>{{ t.sub }}
        </span>
      </div>
    </div>
    <p class="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground" data-testid="status-line">
      <span v-if="line.cold" class="inline-flex items-center gap-1.5">
        <Snowflake class="size-3.5 text-brand" aria-hidden="true" />{{ line.cold }}
      </span>
      <span v-if="line.cold" class="text-faint" aria-hidden="true">·</span>
      <span class="inline-flex items-center gap-1.5">
        <TriangleAlert v-if="line.lossy" class="size-3.5 text-warning" aria-hidden="true" />
        <Check v-else class="size-3.5 text-success" aria-hidden="true" />{{ line.dropped }}
      </span>
    </p>
  </div>
</template>
