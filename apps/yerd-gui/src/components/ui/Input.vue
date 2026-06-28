<script setup lang="ts">
import { ref } from "vue";

import { cn } from "@/lib/utils";

defineProps<{
  modelValue?: string;
  placeholder?: string;
  type?: string;
  disabled?: boolean;
}>();
defineEmits<{ "update:modelValue": [string] }>();

const el = ref<HTMLInputElement | null>(null);

// Exposed so callers (e.g. the ⌘F shortcut) can move focus to the field.
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
      )
    "
    @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  />
</template>
