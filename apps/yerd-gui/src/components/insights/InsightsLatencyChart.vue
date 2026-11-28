<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { CARD } from "@/components/ui/styles";
import type { InsightsPoint } from "@/ipc/types";
import { chartGeometry, formatMs } from "@/lib/insights";
import { cn } from "@/lib/utils";

/**
 * "Latency over time": the warm p50 line over the shaded area under p95, a red
 * dot above every point with failed requests and a marker at the p95 spike.
 * Hand-rolled SVG sized to the card's measured width.
 */
const props = defineProps<{
  series: readonly InsightsPoint[];
  spikeAtMs?: number;
}>();

const HEIGHT = 170;
const box = ref<HTMLElement | null>(null);
const width = ref(640);
let observer: ResizeObserver | null = null;

function measure(): void {
  const w = box.value?.clientWidth ?? 0;
  if (w > 0) width.value = w;
}

onMounted(() => {
  measure();
  if (typeof ResizeObserver === "undefined" || !box.value) return;
  observer = new ResizeObserver(measure);
  observer.observe(box.value);
});

onBeforeUnmount(() => observer?.disconnect());

const geo = computed(() => chartGeometry(props.series, width.value, HEIGHT, props.spikeAtMs));
const empty = computed(() => !props.series.some((p) => p.p50_ms != null || p.errors > 0));
</script>

<template>
  <section :class="cn(CARD, 'flex min-w-0 flex-col px-5 pb-3 pt-4')" data-testid="latency-chart">
    <header class="flex items-center gap-4">
      <h3 class="font-mono text-[11px] uppercase tracking-[1.4px] text-faint">Latency over time</h3>
      <span class="ml-auto flex items-center gap-4 text-xs text-muted-foreground">
        <span class="inline-flex items-center gap-1.5"><span class="h-[3px] w-3 rounded bg-brand" />p50</span>
        <span class="inline-flex items-center gap-1.5"><span class="h-2 w-3 rounded-sm bg-brand/35" />p95</span>
        <span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-destructive" />errors</span>
      </span>
    </header>
    <div ref="box" class="relative mt-3 w-full" :style="{ height: `${HEIGHT}px` }">
      <svg
        :width="geo.width"
        :height="geo.height"
        :viewBox="`0 0 ${geo.width} ${geo.height}`"
        class="block overflow-visible"
        role="img"
        :aria-label="`Latency chart, scale up to ${formatMs(geo.yMax)}`"
      >
        <line
          v-for="y in geo.grid"
          :key="y"
          x1="0"
          :x2="geo.width"
          :y1="y"
          :y2="y"
          class="stroke-border"
          stroke-dasharray="3 4"
        />
        <path :d="geo.p95" class="fill-brand/20 stroke-brand/40" stroke-width="1" data-series="p95" />
        <line
          v-if="geo.spikeX !== null"
          :x1="geo.spikeX"
          :x2="geo.spikeX"
          y1="18"
          :y2="geo.height"
          class="stroke-muted-foreground/50"
          stroke-width="1"
        />
        <path :d="geo.p50" class="fill-none stroke-brand" stroke-width="2" stroke-linejoin="round" data-series="p50" />
        <circle
          v-for="e in geo.errors"
          :key="e.x"
          :cx="e.x"
          :cy="e.y"
          r="3.5"
          class="fill-destructive"
          data-series="errors"
        >
          <title>{{ e.count }} failed</title>
        </circle>
      </svg>
      <p v-if="empty" class="absolute inset-0 flex items-center justify-center text-[13px] text-faint">
        No warm requests in this range yet
      </p>
    </div>
    <div class="relative mt-2 h-4 font-mono text-[11px] text-faint">
      <span
        v-for="(t, i) in geo.ticks"
        :key="i"
        class="absolute top-0 whitespace-nowrap"
        :style="{
          left: `${t.x}px`,
          transform: i === 0 ? 'none' : i === geo.ticks.length - 1 ? 'translateX(-100%)' : 'translateX(-50%)',
        }"
      >
        {{ t.label }}
      </span>
    </div>
  </section>
</template>
