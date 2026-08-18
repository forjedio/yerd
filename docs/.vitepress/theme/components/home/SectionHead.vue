<script setup lang="ts">
// Shared section lead-in: a small mono eyebrow, a display-face heading, and an
// optional supporting line.
defineProps<{ eyebrow: string; heading: string; sub?: string }>()
</script>

<template>
  <header class="y-head">
    <p class="y-head__eyebrow">{{ eyebrow }}</p>
    <h2 class="y-head__heading">{{ heading }}</h2>
    <p v-if="sub" class="y-head__sub">{{ sub }}</p>
  </header>
</template>
