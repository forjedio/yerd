<script setup lang="ts">
/**
 * One Settings group as drawn: a faint uppercase label (with an optional note
 * on the right) over a bordered card whose children are divided rows.
 */
defineProps<{ label: string; note?: string }>();
</script>

<template>
  <section>
    <div class="flex items-baseline gap-4">
      <h2 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">{{ label }}</h2>
      <p v-if="note" class="ml-auto hidden truncate text-[11px] text-faint md:block">{{ note }}</p>
    </div>
    <div class="mt-2 divide-y overflow-hidden rounded-[10px] border bg-card">
      <slot />
    </div>
  </section>
</template>
