<script setup lang="ts">
// One alternating screenshot + copy row for the home page. `reverse` flips the
// image to the right. Renders both theme images; custom.css hides the off-theme
// one (same mechanism as <ThemedImage>).
defineProps<{
  title: string;
  description: string;
  light: string;
  dark: string;
  reverse?: boolean;
}>();
</script>

<template>
  <section class="showcase" :class="{ 'showcase--reverse': reverse }">
    <div class="showcase__media">
      <img :src="light" :alt="title" loading="lazy" class="themed-img--light showcase__img" />
      <img :src="dark" :alt="title" loading="lazy" class="themed-img--dark showcase__img" />
    </div>
    <div class="showcase__body">
      <h3 class="showcase__title">{{ title }}</h3>
      <p class="showcase__text">{{ description }}</p>
    </div>
  </section>
</template>
