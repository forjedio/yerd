<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'

// Sticky chapter strip under the navbar. Highlights the section currently in
// the reading band and scrolls the strip so the active chip stays visible.
const props = defineProps<{ items: { id: string; label: string; number?: string }[] }>()

const active = ref('')
const strip = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null

function reveal(id: string): void {
  const el = strip.value
  const chip = el?.querySelector<HTMLElement>(`[data-id="${id}"]`)
  if (!el || !chip) return
  el.scrollTo({
    left: chip.offsetLeft - (el.clientWidth - chip.offsetWidth) / 2,
    behavior: 'smooth',
  })
}

onMounted(() => {
  observer = new IntersectionObserver(
    (entries) => {
      const hit = entries.find((e) => e.isIntersecting)
      if (!hit || hit.target.id === active.value) return
      active.value = hit.target.id
      reveal(hit.target.id)
    },
    { rootMargin: '-35% 0px -60% 0px' },
  )
  props.items
    .map((item) => document.getElementById(item.id))
    .forEach((el) => el && observer?.observe(el))
})

onUnmounted(() => observer?.disconnect())
</script>

<template>
  <nav class="y-rel-nav" aria-label="Release sections">
    <div ref="strip" class="y-rel-nav__strip">
      <a
        v-for="item in items"
        :key="item.id"
        :data-id="item.id"
        class="y-rel-nav__chip"
        :class="{ 'is-active': active === item.id }"
        :href="`#${item.id}`"
      >
        <span v-if="item.number" class="y-rel-nav__num">{{ item.number }}</span>{{ item.label }}
      </a>
    </div>
  </nav>
</template>
