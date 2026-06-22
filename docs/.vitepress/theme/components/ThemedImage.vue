<script setup lang="ts">
// Renders both the light and dark screenshot; custom.css hides the off-theme
// one via the `.dark` class VitePress sets on <html> before paint, so there's
// no flash on load or on theme switch.
defineProps<{ light: string; dark: string; alt?: string }>();
</script>

<template>
  <img :src="light" :alt="alt" loading="lazy" class="themed-img themed-img--light" />
  <img :src="dark" :alt="alt" loading="lazy" class="themed-img themed-img--dark" />
</template>
