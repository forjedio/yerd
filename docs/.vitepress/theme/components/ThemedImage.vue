<script setup lang="ts">
import { openLightbox } from '../composables/lightbox'

// Renders both the light and dark screenshot; custom.css hides the off-theme
// one via the `.dark` class VitePress sets on <html> before paint, so there's
// no flash on load or on theme switch. Clicking opens it in the global lightbox.
const props = defineProps<{ light: string; dark: string; alt?: string }>();

function open(): void {
  openLightbox({ light: props.light, dark: props.dark, alt: props.alt })
}
</script>

<template>
  <button
    type="button"
    class="themed-img-button"
    :aria-label="alt ? `Enlarge: ${alt}` : 'Enlarge screenshot'"
    @click="open"
  >
    <img :src="light" :alt="alt" loading="lazy" class="themed-img themed-img--light" />
    <img :src="dark" :alt="alt" loading="lazy" class="themed-img themed-img--dark" />
  </button>
</template>
