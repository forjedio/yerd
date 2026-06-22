<script setup lang="ts">
import { AlertTriangle, RefreshCw } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";

/**
 * One async-data surface for every data view. Resolves, in order: loading →
 * spinner; error → an inline message with a Retry button (emits `retry`); empty
 * → the `#empty` slot (use an <EmptyState>); otherwise the default slot.
 *
 * This is what makes the daemon-down / loading / failed / empty stories
 * consistent across views — before this each view rolled its own spinner size,
 * silently swallowed poll errors, or left a blank table after a failed load.
 *
 * Pass `loading` only on the *first* load (no data yet); a background poll
 * refresh should keep showing the last content, not flash a spinner.
 */
defineProps<{
  loading?: boolean;
  /** A human message when the load failed, else null/undefined. */
  error?: string | null;
  /** True when the load succeeded but returned nothing to show. */
  empty?: boolean;
  /** Vertical padding for the loading/error panels (default py-12). */
  pad?: string;
}>();

const emit = defineEmits<{ retry: [] }>();
</script>

<template>
  <div
    v-if="loading"
    class="flex items-center justify-center"
    :class="pad ?? 'py-12'"
  >
    <Spinner class="size-5" />
  </div>

  <div
    v-else-if="error"
    class="flex flex-col items-center justify-center gap-3 text-center"
    :class="pad ?? 'py-12'"
  >
    <AlertTriangle class="size-7 text-destructive" />
    <div>
      <p class="text-sm font-medium">Couldn't load this</p>
      <p class="mx-auto mt-1 max-w-sm text-sm text-muted-foreground">
        {{ error }}
      </p>
    </div>
    <Button variant="outline" size="sm" @click="emit('retry')">
      <RefreshCw class="size-4" /> Try again
    </Button>
  </div>

  <slot v-else-if="empty" name="empty" />

  <slot v-else />
</template>
