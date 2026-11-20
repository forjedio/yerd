<script setup lang="ts">
import type { Component } from "vue";
import { RouterLink } from "vue-router";

/**
 * One 34px sidebar row from the "Yerd App" draft: a 16px monochrome icon and
 * the label. The active row takes the `selected` fill with a brand-tinted icon
 * (plus a faint accent ring in dark mode); inactive rows stay muted until
 * hovered. Colour is reserved for real status in the right-hand slot.
 *
 * The right-hand slot shows at most one of, in priority order:
 *
 * `badge`, a count pill. When `onBadgeClick` is set, clicking the pill runs it
 * instead of navigating the row (a mouse shortcut - keyboard users still reach
 * the same place via the row itself), and `badgeTitle` gives the pill its
 * tooltip.
 *
 * `state`, an "on"/"off" pill in green/red (e.g. Dumps interception).
 *
 * `warn`, an amber dot flagging a row that needs attention (e.g. Doctor when an
 * OS privilege is unelevated), titled by `warnTitle`.
 */
const props = defineProps<{
  to: string;
  label: string;
  icon: Component;
  badge?: number;
  onBadgeClick?: () => void;
  badgeTitle?: string;
  state?: "on" | "off";
  warn?: boolean;
  warnTitle?: string;
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
      class="group flex h-[34px] items-center gap-2.5 rounded-[7px] px-2.5 text-[13.5px] transition-colors"
      :class="
        isActive
          ? 'bg-selected font-semibold text-foreground dark:font-medium dark:ring-1 dark:ring-inset dark:ring-brand/40'
          : 'text-muted-foreground hover:bg-selected/60 hover:text-foreground'
      "
      @click="navigate"
    >
      <component
        :is="icon"
        class="size-4 shrink-0 transition-colors"
        :class="isActive ? 'text-brand' : 'text-muted-foreground group-hover:text-foreground'"
      />
      <span class="min-w-0 truncate">{{ label }}</span>
      <span
        v-if="badge && badge > 0"
        data-testid="nav-badge"
        class="ml-auto flex h-[18px] shrink-0 items-center rounded-[9px] bg-border px-1.5 text-[11px] font-semibold text-foreground tabular-nums"
        :class="onBadgeClick ? 'cursor-pointer hover:bg-border/70' : ''"
        :title="onBadgeClick ? badgeTitle : undefined"
        @click="handleBadge"
      >
        {{ badge > 99 ? "99+" : badge }}
      </span>
      <span
        v-else-if="state"
        data-testid="nav-state"
        class="ml-auto flex h-[18px] shrink-0 items-center rounded-[9px] px-1.5 text-[11px] font-semibold"
        :class="state === 'on' ? 'bg-success/15 text-success' : 'bg-destructive/15 text-destructive'"
      >
        {{ state }}
      </span>
      <span
        v-else-if="warn"
        data-testid="nav-warn"
        class="ml-auto size-2 shrink-0 rounded-full bg-warning"
        :title="warnTitle"
      >
        <span class="sr-only">{{ warnTitle ?? "Needs attention" }}</span>
      </span>
    </a>
  </RouterLink>
</template>
