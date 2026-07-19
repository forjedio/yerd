<script setup lang="ts">
import { computed, type HTMLAttributes } from "vue";
import { TabsList, type TabsListProps, useForwardProps } from "reka-ui";

import { cn } from "@/lib/utils";

const props = defineProps<TabsListProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = computed(() => {
  const { class: _, ...delegated } = props;
  return delegated;
});
const forwarded = useForwardProps(delegatedProps);
</script>

<template>
  <TabsList
    v-bind="forwarded"
    :class="
      cn(
        'flex gap-1 border-border',
        'data-[orientation=horizontal]:flex-wrap data-[orientation=horizontal]:items-center data-[orientation=horizontal]:border-b',
        'data-[orientation=vertical]:flex-col data-[orientation=vertical]:items-stretch data-[orientation=vertical]:border-r',
        props.class,
      )
    "
  >
    <slot />
  </TabsList>
</template>
