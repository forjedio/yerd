<script setup lang="ts">
import { cn } from "@/lib/utils";
</script>

<template>
  <h3 :class="cn('text-base font-semibold leading-none tracking-tight')">
    <slot />
  </h3>
</template>
