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
</script>

<template>
  <TabsTrigger
    v-bind="forwarded"
    :class="
      cn(
        'flex items-center gap-2 whitespace-nowrap border-transparent px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=active]:text-foreground disabled:pointer-events-none disabled:opacity-50',
        'data-[orientation=horizontal]:rounded-t-md data-[orientation=horizontal]:border-b-2 data-[orientation=horizontal]:data-[state=active]:border-primary',
        // Vertical rows fill the rail and keep their label left-aligned, so the
        // active marker is a left edge rather than an underline.
        'data-[orientation=vertical]:rounded-l-md data-[orientation=vertical]:border-l-2 data-[orientation=vertical]:text-left data-[orientation=vertical]:data-[state=active]:border-primary data-[orientation=vertical]:data-[state=active]:bg-accent/50 data-[orientation=vertical]:hover:bg-accent/30',
        props.class,
      )
    "
  >
    <slot />
  </TabsTrigger>
</template>
