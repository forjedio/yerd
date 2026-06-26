<script setup lang="ts">
import type { Component } from "vue";
import { RouterLink } from "vue-router";

/**
 * One sidebar row. The icon is monochrome - muted by default, brand-indigo when
 * active or hovered. Colour is reserved for real status elsewhere; the nav never
 * uses it as decoration. Only the active row tints; the chip backgrounds the old
 * nav painted per-item are gone.
 */
defineProps<{ to: string; label: string; icon: Component }>();
</script>

<template>
  <RouterLink :to="to" custom v-slot="{ isActive, href, navigate }">
    <a
      :href="href"
      :aria-current="isActive ? 'page' : undefined"
      class="group flex items-center gap-2.5 rounded-md px-2 py-1.5 text-sm font-medium transition-colors"
      :class="
        isActive
          ? 'bg-brand/10 text-brand dark:bg-brand/15'
          : 'text-muted-foreground hover:bg-accent hover:text-foreground'
      "
      @click="navigate"
    >
      <component
        :is="icon"
        class="size-4 shrink-0 transition-colors"
        :class="
          isActive
            ? 'text-brand'
            : 'text-muted-foreground/80 group-hover:text-foreground'
        "
      />
      <span class="truncate">{{ label }}</span>
    </a>
  </RouterLink>
</template>
