<script setup lang="ts">
import SectionHead from './SectionHead.vue'
import { openLightbox } from '../../composables/lightbox'

export interface Shot {
  light: string
  dark: string
  alt: string
}

defineProps<{
  eyebrow: string
  heading: string
  sub: string
  primary: Shot
  secondary: Shot[]
  captions: string[]
  chipsLabel: string
  chips: string[]
}>()
</script>

<template>
  <section class="y-section y-glow y-glow--right">
    <SectionHead :eyebrow="eyebrow" :heading="heading" :sub="sub" />

    <div class="y-shots">
      <button
        class="y-shot y-shot--primary"
        type="button"
        :aria-label="primary.alt"
        @click="openLightbox(primary)"
      >
        <img
          class="themed-img--light"
          :src="primary.light"
          :alt="primary.alt"
          loading="lazy"
        />
        <img
          class="themed-img--dark"
          :src="primary.dark"
          :alt="primary.alt"
          loading="lazy"
        />
      </button>

      <div class="y-shots__stack">
        <button
          v-for="shot in secondary"
          :key="shot.light"
          class="y-shot"
          type="button"
          :aria-label="shot.alt"
          @click="openLightbox(shot)"
        >
          <img
            class="themed-img--light"
            :src="shot.light"
            :alt="shot.alt"
            loading="lazy"
          />
          <img
            class="themed-img--dark"
            :src="shot.dark"
            :alt="shot.alt"
            loading="lazy"
          />
        </button>
      </div>
    </div>

    <div class="y-shots__captions">
      <p v-for="caption in captions" :key="caption">{{ caption }}</p>
    </div>

    <p class="y-chips__label">{{ chipsLabel }}</p>
    <ul class="y-chips">
      <li v-for="chip in chips" :key="chip" class="y-tag">
        <span class="y-tag__dot" />{{ chip }}
      </li>
    </ul>
  </section>
</template>
