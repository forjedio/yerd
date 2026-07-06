<script setup lang="ts">
// Click-to-load "facade": renders just the thumbnail + a play button until
// clicked, so a page of videos doesn't pull in YouTube's iframe/JS for
// videos nobody plays. Swaps to the real youtube-nocookie.com embed on click.
import { ref } from 'vue'

const props = defineProps<{ id: string; title: string }>()
const playing = ref(false)

const thumbnail = `https://i.ytimg.com/vi/${props.id}/maxresdefault.jpg`
const embedUrl = `https://www.youtube-nocookie.com/embed/${props.id}?autoplay=1&rel=0`
</script>

<template>
  <div class="yt-embed">
    <button
      v-if="!playing"
      type="button"
      class="yt-embed__facade"
      :aria-label="`Play video: ${title}`"
      @click="playing = true"
    >
      <img :src="thumbnail" :alt="title" loading="lazy" class="yt-embed__thumb" />
      <span class="yt-embed__play" aria-hidden="true">
        <svg viewBox="0 0 68 48" width="68" height="48">
          <path
            d="M66.5 7.7c-.8-2.9-2.5-5.2-5.4-6C55.8.3 34 0 34 0S12.2.3 6.9 1.7c-2.9.8-4.6 3.1-5.4 6C0 13 0 24 0 24s0 11 1.5 16.3c.8 2.9 2.5 5.1 5.4 5.9C12.2 47.7 34 48 34 48s21.8-.3 27.1-1.8c2.9-.8 4.6-3 5.4-5.9C68 35 68 24 68 24s0-11-1.5-16.3z"
            fill="#f00"
          />
          <path d="M45 24 27 14v20" fill="#fff" />
        </svg>
      </span>
    </button>
    <iframe
      v-else
      class="yt-embed__frame"
      :src="embedUrl"
      :title="title"
      loading="lazy"
      allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
      allowfullscreen
    />
  </div>
</template>
