<script setup lang="ts">
import type { Component } from "vue";
import { RouterLink } from "vue-router";

/**
 * One sidebar row. The icon is monochrome - muted by default, brand-indigo when
 * active or hovered. Colour is reserved for real status elsewhere; the nav never
 * uses it as decoration. Only the active row tints; the chip backgrounds the old
 * nav painted per-item are gone.
 *
 * `badge` shows a count pill on the right. When `onBadgeClick` is set, clicking
 * the pill runs it instead of navigating the row (a mouse shortcut - keyboard
 * users still reach the same place via the row itself), and `badgeTitle` gives
 * the pill its tooltip.
 */
const props = defineProps<{
  to: string;
  label: string;
  icon: Component;
  badge?: number;
  onBadgeClick?: () => void;
  badgeTitle?: string;
}>();

function handleBadge(e: MouseEvent): void {
  if (!props.onBadgeClick) return;
  e.stopPropagation();
  e.preventDefault();
  props.onBadgeClick();
}
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
      <span class="min-w-0 truncate">{{ label }}</span>
      <span
        v-if="badge && badge > 0"
        class="ml-auto shrink-0 rounded-full bg-brand px-1.5 py-0.5 text-[10px] font-semibold leading-none text-white tabular-nums"
        :class="onBadgeClick ? 'cursor-pointer hover:bg-brand/80' : ''"
        :title="onBadgeClick ? badgeTitle : undefined"
        @click="handleBadge"
      >
        {{ badge > 99 ? "99+" : badge }}
      </span>
    </a>
  </RouterLink>
</template>
