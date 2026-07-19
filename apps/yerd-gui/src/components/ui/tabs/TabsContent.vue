<script setup lang="ts">
import { computed, type HTMLAttributes } from "vue";
import { TabsContent, type TabsContentProps, useForwardProps } from "reka-ui";

import { cn } from "@/lib/utils";

const props = defineProps<
  TabsContentProps & { class?: HTMLAttributes["class"] }
>();

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props;
  return delegated;
});
const forwarded = useForwardProps(delegatedProps);
</script>

<template>
  <TabsContent
    v-bind="forwarded"
    :class="
      cn(
        'focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        'data-[orientation=horizontal]:pt-4',
        props.class,
      )
    "
  >
    <slot />
  </TabsContent>
</template>
