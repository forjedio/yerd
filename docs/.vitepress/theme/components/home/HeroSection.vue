<script setup lang="ts">
import type { Shot } from './AppSection.vue'

export interface HeroAction {
  text: string
  link: string
  theme?: 'brand' | 'alt'
}

defineProps<{
  pill: string
  announce?: { badge: string; text: string; link: string }
  lines: string[]
  accent: string
  tagline: string
  actions: HeroAction[]
  platforms: string
  shot: Shot
}>()
</script>

<template>
  <section class="y-hero y-glow y-glow--hero">
    <div class="y-hero__inner">
      <div class="y-hero__copy">
        <a v-if="announce" class="y-pill y-pill--link" :href="announce.link">
          <span class="y-pill__badge">{{ announce.badge }}</span>{{ announce.text }}
          <span class="y-pill__arrow" aria-hidden="true">→</span>
        </a>
        <p v-else class="y-pill"><span class="y-pill__dot" />{{ pill }}</p>

        <h1 class="y-hero__title">
          <span v-for="line in lines.slice(0, -1)" :key="line">{{ line }}</span>
          <span>{{ lines.at(-1) }} <span class="y-hero__accent">{{ accent }}</span></span>
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

        <p class="y-hero__platforms">{{ platforms }}</p>
      </div>

      <div class="y-hero__demo">
        <div class="y-hero__shot">
          <img class="themed-img--light" :src="shot.light" :alt="shot.alt" />
          <img class="themed-img--dark" :src="shot.dark" :alt="shot.alt" />
        </div>
      </div>
    </div>
  </section>
</template>
