<script setup lang="ts">
import { Loader2 } from "lucide-vue-next";

import { cn } from "@/lib/utils";

defineProps<{ class?: string }>();
</script>

<template>
  <Loader2 :class="cn('animate-spin text-muted-foreground', $props.class)" />
</template>
