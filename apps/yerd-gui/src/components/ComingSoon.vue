<script setup lang="ts">
import { cn } from "@/lib/utils";

/**
 * A deliberately-disabled affordance for features that need a daemon-side IPC
 * that does not exist yet (log viewing, daemon restart, per-service restart).
 * Renders greyed and non-interactive with an explanatory native tooltip, so the
 * gap reads as intentional rather than broken. See the plan's "Coming soon" set.
 */
withDefaults(
  defineProps<{
    /** Why it's disabled — shown on hover. */
    reason?: string;
    /** Render as a small inline pill instead of a button-sized control. */
    pill?: boolean;
  }>(),
  {
    reason: "Needs a daemon IPC that isn't built yet — coming soon.",
    pill: false,
  },
);
</script>

<template>
  <span
    :title="reason"
    aria-disabled="true"
    :class="
      cn(
        'inline-flex cursor-not-allowed select-none items-center gap-1.5 rounded-md border border-dashed border-input text-muted-foreground opacity-70',
        pill ? 'px-2 py-0.5 text-xs' : 'h-8 px-3 text-xs font-medium',
      )
    "
  >
    <slot />
    <span class="rounded bg-muted px-1 text-[10px] uppercase tracking-wide"
      >soon</span
    >
  </span>
</template>
