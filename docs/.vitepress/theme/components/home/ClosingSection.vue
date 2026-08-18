<script setup lang="ts">
import type { HeroAction } from './HeroSection.vue'

defineProps<{ lines: string[]; sub: string; actions: HeroAction[] }>()
</script>

<template>
  <section class="y-close y-glow">
    <h2 class="y-close__heading">
      <span v-for="line in lines" :key="line">{{ line + ' ' }}</span>
    </h2>
    <p class="y-close__sub">{{ sub }}</p>
    <div class="y-close__actions">
      <a
        v-for="action in actions"
        :key="action.link"
        class="y-btn"
        :class="action.theme === 'alt' ? 'y-btn--alt' : 'y-btn--brand'"
        :href="action.link"
        >{{ action.text }}</a
      >
    </div>
  </section>
</template>
