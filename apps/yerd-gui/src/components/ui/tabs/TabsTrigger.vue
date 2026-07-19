<script setup lang="ts">
import { computed, type HTMLAttributes } from "vue";
import { TabsTrigger, type TabsTriggerProps, useForwardProps } from "reka-ui";

import { cn } from "@/lib/utils";

const props = defineProps<
  TabsTriggerProps & { class?: HTMLAttributes["class"] }
>();

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props;
  return delegated;
});
const forwarded = useForwardProps(delegatedProps);

/**
 * Orientation-aware styling. Horizontal triggers underline the active tab;
 * vertical ones fill the rail, keep their label left-aligned, and mark the
 * active row with a left edge instead. reka puts `data-orientation` on every
 * trigger, so the two sets never both apply.
 */
const triggerClass =
  "flex items-center gap-2 whitespace-nowrap border-transparent px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=active]:text-foreground disabled:pointer-events-none disabled:opacity-50 " +
  "data-[orientation=horizontal]:rounded-t-md data-[orientation=horizontal]:border-b-2 data-[orientation=horizontal]:data-[state=active]:border-primary " +
  "data-[orientation=vertical]:rounded-l-md data-[orientation=vertical]:border-l-2 data-[orientation=vertical]:text-left data-[orientation=vertical]:data-[state=active]:border-primary data-[orientation=vertical]:data-[state=active]:bg-accent/50 data-[orientation=vertical]:hover:bg-accent/30";
</script>

<template>
  <TabsTrigger
    v-bind="forwarded"
    :class="cn(triggerClass, props.class)"
  >
    <slot />
  </TabsTrigger>
</template>
