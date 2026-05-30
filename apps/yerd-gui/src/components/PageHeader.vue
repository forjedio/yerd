<script setup lang="ts">
defineProps<{ title: string; subtitle?: string }>();
</script>

<template>
  <header
    class="sticky top-0 z-10 flex items-center justify-between gap-4 border-b bg-background/80 px-6 py-4 backdrop-blur"
  >
    <div>
      <h2 class="text-xl font-semibold tracking-tight">{{ title }}</h2>
      <p v-if="subtitle" class="text-sm text-muted-foreground">{{ subtitle }}</p>
    </div>
    <div class="flex items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
