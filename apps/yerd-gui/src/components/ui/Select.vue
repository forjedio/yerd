<script setup lang="ts" generic="T extends string">
import { cn } from "@/lib/utils";

defineProps<{
  modelValue: T;
  options: readonly { value: T; label: string }[];
  disabled?: boolean;
  ariaLabel?: string;
}>();
defineEmits<{ "update:modelValue": [T] }>();
</script>

<template>
  <select
    :value="modelValue"
    :disabled="disabled"
    :aria-label="ariaLabel"
    :class="
      cn(
        'h-8 rounded-md border border-input bg-background px-2 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
      )
    "
    @change="
      $emit('update:modelValue', ($event.target as HTMLSelectElement).value as T)
    "
  >
    <option v-for="o in options" :key="o.value" :value="o.value">
      {{ o.label }}
    </option>
  </select>
</template>
