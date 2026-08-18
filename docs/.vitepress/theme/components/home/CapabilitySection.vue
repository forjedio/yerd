<script setup lang="ts">
import HomeIcon from './HomeIcon.vue'
import SectionHead from './SectionHead.vue'

defineProps<{
  eyebrow: string
  heading: string
  sub: string
  items: {
    icon: string
    title: string
    details: string
    command: string
    link?: string
  }[]
}>()
</script>

<template>
  <section class="y-section y-glow y-glow--left">
    <SectionHead :eyebrow="eyebrow" :heading="heading" :sub="sub" />

    <ul class="y-cards">
      <li v-for="item in items" :key="item.title" class="y-card">
        <component
          :is="item.link ? 'a' : 'div'"
          :href="item.link"
          class="y-card__body"
        >
          <HomeIcon class="y-card__icon" :name="item.icon" />
          <h3 class="y-card__title">{{ item.title }}</h3>
          <p class="y-card__text">{{ item.details }}</p>
          <code class="y-chip">{{ item.command }}</code>
        </component>
      </li>
    </ul>
  </section>
</template>
