<script setup lang="ts">
import { computed } from 'vue'
import SectionHead from './SectionHead.vue'

// The install-size comparison bar. The track stands for `scaleMax` megabytes,
// and each bar is drawn as a percentage of it - so the numbers in the
// frontmatter are the only thing to keep truthful.
const props = defineProps<{
  eyebrow: string
  heading: string
  sub: string
  scaleMax: number
  ticks: string[]
  yerd: { label: string; mb: number }
  middle: { label: string; mb: number }
  full: { label: string }
}>()

const yerdWidth = computed(
  () => `max(6px, ${(props.yerd.mb / props.scaleMax) * 100}%)`,
)
const middleWidth = computed(
  () => `${(props.middle.mb / props.scaleMax) * 100}%`,
)

// Ticks are pinned to their position on the track: the first hugs the left
// edge, the last the right, and the rest centre on their mark.
function tickClass(i: number): string {
  if (i === 0) return 'is-first'
  return i === props.ticks.length - 1 ? 'is-last' : ''
}

function tickStyle(i: number): Record<string, string> {
  if (i === 0 || i === props.ticks.length - 1) return {}
  return { left: `${(i / (props.ticks.length - 1)) * 100}%` }
}
</script>

<template>
  <section class="y-section">
    <SectionHead :eyebrow="eyebrow" :heading="heading" :sub="sub" />

    <div class="y-fp">
      <div class="y-fp__track">
        <div class="y-fp__middle" :style="{ width: middleWidth }" />
        <div class="y-fp__yerd" :style="{ width: yerdWidth }" />
      </div>

      <div class="y-fp__scale">
        <span
          v-for="(tick, i) in ticks"
          :key="tick"
          class="y-fp__tick"
          :class="tickClass(i)"
          :style="tickStyle(i)"
          >{{ tick }}</span
        >
      </div>

      <div class="y-fp__keys">
        <span class="y-fp__key y-fp__key--brand">{{ yerd.label }}</span>
        <span class="y-fp__key">{{ middle.label }}</span>
        <span class="y-fp__key y-fp__key--end">{{ full.label }}</span>
      </div>
    </div>
  </section>
</template>
