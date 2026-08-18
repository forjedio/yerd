<script setup lang="ts">
import SectionHead from './SectionHead.vue'

defineProps<{
  eyebrow: string
  heading: string
  items: { command: string; details: string }[]
}>()
</script>

<template>
  <section class="y-section">
    <SectionHead :eyebrow="eyebrow" :heading="heading" />

    <ol class="y-steps">
      <li v-for="(item, i) in items" :key="item.command" class="y-step">
        <span class="y-step__badge">{{ String(i + 1).padStart(2, '0') }}</span>
        <code class="y-chip y-chip--lg">{{ item.command }}</code>
        <p class="y-step__text">{{ item.details }}</p>
      </li>
    </ol>
  </section>
</template>
