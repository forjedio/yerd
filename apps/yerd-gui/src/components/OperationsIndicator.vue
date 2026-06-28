<script setup lang="ts">
import { computed } from "vue";

import Spinner from "@/components/ui/Spinner.vue";
import { useOperations } from "@/composables/useOperations";

// Global "something is running" line for the SideNav footer. Long operations
// (daemon start, PHP install, …) register in the singleton `useOperations`, so
// they stay visible here even after the user navigates away from the screen that
// started them. Shows the first active op's label + detail, with a "+N" when
// several run at once. Renders nothing when idle.
const { active } = useOperations();

const primary = computed(() => active.value[0] ?? null);
const extra = computed(() => Math.max(0, active.value.length - 1));
</script>

<template>
  <div v-if="primary" class="mb-2 flex items-center gap-2">
    <Spinner class="size-3.5 shrink-0" />
    <div class="min-w-0 flex-1">
      <p class="truncate text-xs font-medium">
        {{ primary.label }}
        <span v-if="extra" class="font-normal text-muted-foreground">+{{ extra }}</span>
      </p>
      <p v-if="primary.detail" class="truncate text-[11px] text-muted-foreground">
        {{ primary.detail }}
      </p>
    </div>
  </div>
</template>
