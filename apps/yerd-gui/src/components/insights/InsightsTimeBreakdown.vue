<script setup lang="ts">
import { computed } from "vue";

import { CARD } from "@/components/ui/styles";
import type { InsightsSlice } from "@/ipc/types";
import { breakdownBar, formatMs } from "@/lib/insights";
import { cn } from "@/lib/utils";

/**
 * "Where the time goes": the average warm request split into application,
 * database, outgoing HTTP and worker wait, as a stacked bar and a legend.
 */
const props = defineProps<{ slices: readonly InsightsSlice[] }>();

const segments = computed(() =>
  breakdownBar(props.slices.map((s) => ({ part: s.part, ms: s.mean_ms, share: s.share }))),
);
</script>

<template>
  <section :class="cn(CARD, 'flex min-w-0 flex-col px-5 pb-4 pt-4')" data-testid="time-breakdown">
    <h3 class="font-mono text-[11px] uppercase tracking-[1.4px] text-faint">Where the time goes</h3>
    <template v-if="segments.length">
      <div class="mt-5 flex h-2 gap-0.5 overflow-hidden rounded-full" aria-hidden="true">
        <span
          v-for="s in segments"
          :key="s.part"
          :class="cn('h-full first:rounded-l-full last:rounded-r-full', s.fill)"
          :style="{ width: `${(s.share * 100).toFixed(2)}%` }"
        />
      </div>
      <ul class="mt-4 flex flex-col gap-2.5">
        <li v-for="s in segments" :key="s.part" class="flex items-center gap-2.5 text-sm" :data-part="s.part">
          <span :class="cn('size-2 shrink-0 rounded-sm', s.fill)" aria-hidden="true" />
          <span class="min-w-0 flex-1 truncate">{{ s.label }}</span>
          <span class="shrink-0 font-mono text-[13px] text-muted-foreground">
            {{ Math.round(s.share * 100) }}% · {{ formatMs(s.ms) }}
          </span>
        </li>
      </ul>
    </template>
    <p v-else class="mt-5 text-[13px] text-faint">No warm requests in this range yet.</p>
    <p class="mt-auto pt-4 text-xs leading-relaxed text-faint">
      Average warm request. Database and HTTP time come from the Yerd PHP extension; the rest is measured by the
      proxy.
    </p>
  </section>
</template>
