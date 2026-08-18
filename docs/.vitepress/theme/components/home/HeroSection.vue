<script setup lang="ts">
import CopyCommand from './CopyCommand.vue'
import { openLightbox } from '../../composables/lightbox'
import type { Shot } from './AppSection.vue'

export interface HeroAction {
  text: string
  link: string
  theme?: 'brand' | 'alt'
}

defineProps<{
  pill: string
  lines: string[]
  accent: string
  tagline: string
  command: { label: string; text: string }
  actions: HeroAction[]
  platforms: string
  shot: Shot
}>()
</script>

<template>
  <section class="y-hero y-glow y-glow--hero">
    <div class="y-hero__inner">
      <div class="y-hero__copy">
        <p class="y-pill"><span class="y-pill__dot" />{{ pill }}</p>

        <h1 class="y-hero__title">
          <span v-for="line in lines" :key="line">{{ line + ' ' }}</span>
          <span class="y-hero__accent">{{ accent }}</span>
        </h1>

        <p class="y-hero__tagline">{{ tagline }}</p>

        <CopyCommand :label="command.label" :command="command.text" />

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

        <p class="y-hero__platforms">{{ platforms }}</p>
      </div>

      <div class="y-hero__demo">
        <button
          class="y-hero__shot"
          type="button"
          :aria-label="shot.alt"
          @click="openLightbox(shot)"
        >
          <img class="themed-img--light" :src="shot.light" :alt="shot.alt" />
          <img class="themed-img--dark" :src="shot.dark" :alt="shot.alt" />
        </button>
      </div>
    </div>
  </section>
</template>
