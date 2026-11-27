<script setup lang="ts">
import { ref, type PropType } from 'vue'
import type { Shot } from './types'

// A before/after slider. The "after" shot fills the frame and the "before" shot
// is clipped to the left of the handle; a visually hidden range input drives it
// so the comparison works with the keyboard and screen readers too.
type LabelledShot = Shot & { label: string }

defineProps({
  before: { type: Object as PropType<LabelledShot>, required: true },
  after: { type: Object as PropType<LabelledShot>, required: true },
})

const position = ref(50)
</script>

<template>
  <div class="y-compare" :style="{ '--y-pos': `${position}%` }">
    <div class="y-compare__layer">
      <img class="themed-img--light" :src="after.light" :alt="after.alt" loading="lazy" />
      <img class="themed-img--dark" :src="after.dark" :alt="after.alt" loading="lazy" />
    </div>
    <div class="y-compare__layer y-compare__layer--before">
      <img class="themed-img--light" :src="before.light" :alt="before.alt" loading="lazy" />
      <img class="themed-img--dark" :src="before.dark" :alt="before.alt" loading="lazy" />
    </div>

    <span class="y-compare__label y-compare__label--before">{{ before.label }}</span>
    <span class="y-compare__label y-compare__label--after">{{ after.label }}</span>

    <span class="y-compare__handle" aria-hidden="true">
      <span class="y-compare__knob">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M9 6l-6 6 6 6M15 6l6 6-6 6" />
        </svg>
      </span>
    </span>

    <input
      v-model.number="position"
      class="y-compare__range"
      type="range"
      min="0"
      max="100"
      step="0.5"
      :aria-label="`Drag to compare ${before.label} with ${after.label}`"
    />
  </div>
</template>
