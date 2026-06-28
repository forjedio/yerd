<script setup lang="ts">
import DefaultTheme from 'vitepress/theme'
import Lightbox from './components/Lightbox.vue'

// Wrap the default theme layout so the single global <Lightbox> overlay is
// mounted once for every page (via the `layout-bottom` slot).
const { Layout } = DefaultTheme
</script>

<template>
  <Layout>
    <!-- Social-card-style hero lockup: logo + "Yerd" wordmark, then the headline
         with a blue accent on "without the friction." -->
    <template #home-hero-info>
      <h1 class="heading">
        <span class="hero-brand">
          <img class="hero-logo" src="/logo.svg" alt="" />
          <span class="name">Yerd</span>
        </span>
        <span class="text"
          >Local PHP, <span class="accent">without the friction.</span></span
        >
      </h1>
      <p class="tagline">
        Serve your projects on .test domains over HTTP and HTTPS, run a different
        PHP version per site, and manage it all from one tiny daemon. No Docker,
        no sudo for everyday work, no subscription.
      </p>
    </template>
    <!-- Hero screenshot, with light/dark variants toggled by CSS on html.dark. -->
    <template #home-hero-image>
      <img
        class="hero-shot hero-shot--light"
        src="/images/overview-light.png"
        alt="The Yerd desktop app"
      />
      <img
        class="hero-shot hero-shot--dark"
        src="/images/overview-dark.png"
        alt="The Yerd desktop app"
      />
    </template>
    <template #layout-bottom>
      <Lightbox />
    </template>
  </Layout>
</template>
