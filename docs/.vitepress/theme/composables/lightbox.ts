import { ref } from 'vue'

// Shared lightbox state. Any image (e.g. <ShowcaseRow>) calls openLightbox()
// with its light/dark sources; the single global <Lightbox> overlay renders it.
export interface LightboxImage {
  light: string
  dark: string
  alt?: string
}

export const lightboxImage = ref<LightboxImage | null>(null)

export function openLightbox(img: LightboxImage): void {
  lightboxImage.value = img
}

export function closeLightbox(): void {
  lightboxImage.value = null
}
