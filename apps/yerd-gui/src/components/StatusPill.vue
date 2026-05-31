<script setup lang="ts">
import { computed } from "vue";

import { cn } from "@/lib/utils";

export type Tone = "ok" | "warn" | "bad" | "unknown" | "muted";

const props = defineProps<{ tone: Tone; label: string; pulse?: boolean }>();

const dot = computed(
  () =>
    ({
      ok: "bg-success",
      warn: "bg-warning",
      // The dark-mode --destructive token is deliberately dark (for button fills);
      // a status dot needs a brighter red to read on a dark background.
      bad: "bg-destructive dark:bg-red-500",
      unknown: "bg-muted-foreground",
      muted: "bg-muted-foreground/50",
    })[props.tone],
);
</script>

<template>
  <span class="inline-flex items-center gap-1.5 text-xs">
    <span class="relative flex size-2">
      <span
        v-if="pulse && tone === 'ok'"
        :class="
          cn('absolute inline-flex h-full w-full animate-ping rounded-full opacity-75', dot)
        "
      />
      <span :class="cn('relative inline-flex size-2 rounded-full', dot)" />
    </span>
    <span class="text-muted-foreground">{{ label }}</span>
  </span>
</template>
