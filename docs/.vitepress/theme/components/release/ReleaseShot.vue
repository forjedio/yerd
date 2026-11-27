<script setup lang="ts">
import { openLightbox } from '../../composables/lightbox'
import type { PropType } from 'vue'
import type { Shot } from './types'

// A framed light/dark screenshot that opens in the global lightbox.
const props = defineProps({
  shot: { type: Object as PropType<Shot>, required: true },
  eager: { type: Boolean, default: false },
})

function open(): void {
  openLightbox(props.shot)
}
</script>

<template>
  <button
    type="button"
    class="y-rel-shot"
    :aria-label="`Enlarge: ${shot.alt}`"
    @click="open"
  >
    <img
      class="themed-img--light"
      :src="shot.light"
      :alt="shot.alt"
      :loading="eager ? 'eager' : 'lazy'"
    />
    <img
      class="themed-img--dark"
      :src="shot.dark"
      :alt="shot.alt"
      :loading="eager ? 'eager' : 'lazy'"
    />
  </button>
</template>
