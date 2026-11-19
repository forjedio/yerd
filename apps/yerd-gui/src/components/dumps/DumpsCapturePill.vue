<script setup lang="ts">
import { Pause } from "lucide-vue-next";
import { computed } from "vue";

import type { DumpsStatusResponse } from "@/ipc/types";
import { captureState, pillLabel } from "@/lib/dumps";
import { cn } from "@/lib/utils";

/**
 * The toolbar's capture state: "Intercepting on {port}" (compact "Live ·
 * {port}"), "Paused", or "Dump server stopped" when the port could not be
 * bound. Renders nothing until the first status arrives.
 */
const props = defineProps<{
  status: DumpsStatusResponse | null;
  compact?: boolean;
}>();

const state = computed(() => captureState(props.status));
const port = computed(() => props.status?.port ?? 0);
const label = computed(() => pillLabel(state.value, port.value, props.compact ?? false));
const title = computed(() => (state.value === "stopped" ? `Port ${port.value} in use, retrying` : undefined));
</script>

<template>
  <span
    v-if="state !== 'unknown'"
    :class="
      cn(
        'inline-flex h-7 shrink-0 items-center gap-2 rounded-full px-3 text-xs font-semibold',
        state === 'live' && 'bg-success/15 text-success',
        state === 'paused' && 'border text-muted-foreground',
        state === 'stopped' && 'border border-destructive/40 text-destructive',
      )
    "
    :title="title"
    :data-state="state"
    data-testid="capture-pill"
  >
    <Pause v-if="state === 'paused'" class="size-3.5" aria-hidden="true" />
    <span
      v-else
      :class="cn('size-[7px] rounded-full', state === 'live' ? 'bg-success' : 'bg-destructive')"
      aria-hidden="true"
    />
    {{ label }}
  </span>
</template>
