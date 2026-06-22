<script setup lang="ts">
import type { Component } from "vue";

/**
 * The one empty-state surface: a dashed-border panel with an icon, a title, an
 * optional sentence, and a slot for a call-to-action. Replaces the per-view
 * hand-rolled "No X yet" panels so every empty list reads the same and always
 * invites the next action rather than showing a blank table. Copy should be an
 * invitation ("No sites yet — park a folder to get started"), never an apology.
 */
defineProps<{ icon?: Component; title: string; description?: string }>();
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed px-6 py-10 text-center"
  >
    <component :is="icon" v-if="icon" class="size-7 text-muted-foreground" />
    <div>
      <p class="text-sm font-medium">{{ title }}</p>
      <p
        v-if="description"
        class="mx-auto mt-1 max-w-sm text-sm text-muted-foreground"
      >
        {{ description }}
      </p>
    </div>
    <div v-if="$slots.default" class="mt-1"><slot /></div>
  </div>
</template>
