<script setup lang="ts">
import { ref, type HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

/**
 * A text field. `class` is a prop rather than a fallthrough attribute so a
 * caller's utilities are merged with `cn` and replace the conflicting defaults
 * (height, padding, disabled opacity) instead of racing them in CSS order.
 */
const props = defineProps<{
  modelValue?: string;
  placeholder?: string;
  type?: string;
  disabled?: boolean;
  class?: HTMLAttributes["class"];
}>();
defineEmits<{ "update:modelValue": [string] }>();

const el = ref<HTMLInputElement | null>(null);

defineExpose({ focus: () => el.value?.focus() });
</script>

<template>
  <input
    ref="el"
    :type="type ?? 'text'"
    :value="modelValue"
    :placeholder="placeholder"
    :disabled="disabled"
    :class="
      cn(
        'flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
        props.class,
      )
    "
    @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  />
</template>
