<script setup lang="ts">
import type { HeroAction } from '../home/HeroSection.vue'
import ReleaseShot from './ReleaseShot.vue'
import type { PropType } from 'vue'
import type { Shot } from './types'

// Release hero: the headline, and three screenshots fanned out beneath it
// with the centre one in front.
defineProps({
  pill: { type: String, required: true },
  lines: { type: Array as PropType<string[]>, required: true },
  accent: { type: String, required: true },
  tagline: { type: String, required: true },
  actions: { type: Array as PropType<HeroAction[]>, required: true },
  shots: {
    type: Object as PropType<{ left: Shot; centre: Shot; right: Shot }>,
    required: true,
  },
})
</script>

<template>
  <section class="y-rel-hero y-glow y-glow--hero">
    <div class="y-rel-hero__inner">
      <p class="y-pill"><span class="y-pill__dot y-pill__dot--pulse" />{{ pill }}</p>

      <h1 class="y-hero__title">
        <span v-for="line in lines" :key="line">{{ line }}</span>
        <span class="y-hero__accent">{{ accent }}</span>
      </h1>

      <p class="y-hero__tagline">{{ tagline }}</p>

      <div class="y-hero__actions">
        <a
          v-for="action in actions"
          :key="action.link"
          class="y-btn"
          :class="action.theme === 'alt' ? 'y-btn--alt' : 'y-btn--brand'"
          :href="action.link"
          >{{ action.text }}</a
        >
      </div>
    </div>

    <div class="y-fan">
      <ReleaseShot :shot="shots.left" class="y-fan__shot y-fan__shot--left" />
      <ReleaseShot :shot="shots.right" class="y-fan__shot y-fan__shot--right" />
      <ReleaseShot :shot="shots.centre" class="y-fan__shot y-fan__shot--centre" eager />
    </div>
  </section>
</template>
