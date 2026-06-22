<script setup lang="ts">
import DefaultTheme from 'vitepress/theme'
import Lightbox from './components/Lightbox.vue'

// Wrap the default theme layout so the single global <Lightbox> overlay is
// mounted once for every page (via the `layout-bottom` slot).
const { Layout } = DefaultTheme
</script>

<template>
  <Layout>
    <template #layout-bottom>
      <Lightbox />
    </template>
  </Layout>
</template>
