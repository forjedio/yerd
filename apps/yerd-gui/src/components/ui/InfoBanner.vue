<script lang="ts">
import { cva, type VariantProps } from "class-variance-authority";

export const infoBannerVariants = cva(
  "flex items-start gap-2 rounded-md border px-3 py-2 text-xs leading-relaxed",
  {
    variants: {
      variant: {
        info: "border-border bg-muted/50 text-muted-foreground",
        warning: "border-warning/40 bg-warning/10 text-warning",
        destructive: "border-destructive/40 bg-destructive/10 text-destructive",
      },
    },
    defaultVariants: { variant: "info" },
  },
);

export type InfoBannerVariants = VariantProps<typeof infoBannerVariants>;
</script>

<script setup lang="ts">
import { CircleAlert, Info, TriangleAlert } from "lucide-vue-next";
import { computed, type Component } from "vue";

import { cn } from "@/lib/utils";

/**
 * A short note attached to a control: what a field accepts, a soft warning, or
 * why an entry was rejected. One surface for all three so a panel doesn't drift
 * into a mix of bare muted paragraphs and hand-rolled coloured boxes, and so a
 * note that changes severity keeps its footprint instead of reflowing the form.
 *
 * Content goes in the default slot, which may carry markup (`<code>` and the
 * like). `icon` overrides the per-variant glyph where a more specific one
 * carries meaning.
 */
const props = defineProps<{
  variant?: InfoBannerVariants["variant"];
  icon?: Component;
}>();

const VARIANT_ICONS = {
  info: Info,
  warning: TriangleAlert,
  destructive: CircleAlert,
} as const;

const glyph = computed(() => props.icon ?? VARIANT_ICONS[props.variant ?? "info"]);

/** A destructive banner reports a failure the user needs told about now (a
 *  rejected entry), so it announces itself; the calmer variants are static
 *  guidance a screen reader should reach in document order. */
const role = computed(() => (props.variant === "destructive" ? "alert" : undefined));
</script>

<template>
  <div :class="cn(infoBannerVariants({ variant }))" :role="role">
    <component :is="glyph" class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
    <div class="min-w-0"><slot /></div>
  </div>
</template>
