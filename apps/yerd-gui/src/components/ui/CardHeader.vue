<script setup lang="ts">
import { cn } from "@/lib/utils";
</script>

<template>
  <div :class="cn('flex flex-col space-y-1.5 p-5')"><slot /></div>
</template>
