<script setup lang="ts">
// The line icons used on the capability cards. Kept inline (rather than an icon
// dependency) so the home page ships no extra runtime and the strokes inherit
// the card's colour.
defineProps<{ name: string }>()

const paths: Record<string, string[]> = {
  grid: ['M3 3h7v7H3z', 'M14 3h7v7h-7z', 'M3 14h7v7H3z', 'M14 14h7v7h-7z'],
  lock: ['M5 11h14v10H5z', 'M8 11V7a4 4 0 0 1 8 0v4'],
  stack: ['M12 3 2 8l10 5 10-5-10-5z', 'M2 16l10 5 10-5', 'M2 12l10 5 10-5'],
  database: [
    'M12 3c4.4 0 8 1.3 8 3s-3.6 3-8 3-8-1.3-8-3 3.6-3 8-3z',
    'M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6',
    'M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3',
  ],
  mail: ['M3 5h18v14H3z', 'M3 7l9 6 9-6'],
  shield: ['M12 3l8 3v6c0 5-3.4 8.2-8 9-4.6-.8-8-4-8-9V6l8-3z', 'M9 12l2 2 4-4'],
}
</script>

<template>
  <svg
    class="y-icon"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.6"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path v-for="(d, i) in paths[name] ?? []" :key="i" :d="d" />
  </svg>
</template>
