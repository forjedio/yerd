<script setup lang="ts">
import { openLightbox } from '../composables/lightbox'

// One alternating screenshot + copy row for the home page. `reverse` flips the
// image to the right. Renders both theme images; custom.css hides the off-theme
// one (same mechanism as <ThemedImage>). Clicking the image opens the lightbox.
const props = defineProps<{
  title: string
  description: string
  light: string
  dark: string
  reverse?: boolean
}>()

function zoom(): void {
  openLightbox({ light: props.light, dark: props.dark, alt: props.title })
}
</script>

<template>
  <section class="showcase" :class="{ 'showcase--reverse': reverse }">
    <div class="showcase__media">
      <img
        :src="light"
        :alt="title"
        loading="lazy"
        class="themed-img--light showcase__img"
        @click="zoom"
      />
      <img
        :src="dark"
        :alt="title"
        loading="lazy"
        class="themed-img--dark showcase__img"
        @click="zoom"
      />
    </div>
    <div class="showcase__body">
      <h3 class="showcase__title">{{ title }}</h3>
      <p class="showcase__text">{{ description }}</p>
    </div>
  </section>
</template>
