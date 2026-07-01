<script setup lang="ts">
import { type HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

const props = defineProps<{ class?: HTMLAttributes["class"] }>();
</script>

<template>
  <h3 :class="cn('font-display text-lg font-normal tracking-wide', props.class)">
    <slot />
  </h3>
</template>
