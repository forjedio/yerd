<script setup lang="ts">
import { onMounted, onUnmounted, watch } from 'vue'
import { closeLightbox, lightboxImage } from '../composables/lightbox'

// Full-screen image viewer. Renders both theme images (custom.css hides the
// off-theme one, same as <ThemedImage>). Close on backdrop click, the × button,
// or Escape; body scroll is locked while open.
function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape') closeLightbox()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  document.documentElement.style.overflow = ''
})

watch(lightboxImage, (img) => {
  if (typeof document !== 'undefined') {
    document.documentElement.style.overflow = img ? 'hidden' : ''
  }
})
</script>

<template>
  <ClientOnly>
    <Teleport to="body">
      <Transition name="lightbox-fade">
        <div
          v-if="lightboxImage"
          class="lightbox"
          role="dialog"
          aria-modal="true"
          @click="closeLightbox"
        >
          <button class="lightbox__close" aria-label="Close" @click.stop="closeLightbox">
            &times;
          </button>
          <img
            :src="lightboxImage.light"
            :alt="lightboxImage.alt"
            class="lightbox__img themed-img--light"
            @click.stop
          />
          <img
            :src="lightboxImage.dark"
            :alt="lightboxImage.alt"
            class="lightbox__img themed-img--dark"
            @click.stop
          />
        </div>
      </Transition>
    </Teleport>
  </ClientOnly>
</template>
