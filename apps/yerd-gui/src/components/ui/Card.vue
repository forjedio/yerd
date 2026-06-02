<script setup lang="ts">
import { type HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

const props = defineProps<{ class?: HTMLAttributes["class"] }>();
</script>

<template>
  <div :class="cn('rounded-lg border p-5', props.class)">
    <slot />
  </div>
</template>
