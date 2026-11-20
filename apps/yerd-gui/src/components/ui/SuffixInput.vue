<script setup lang="ts">
import { computed, ref } from "vue";

import { cn } from "@/lib/utils";

/**
 * A mono text field with a faint suffix that follows the typed text, as the
 * draft draws site names (`invoicer .test`). The input is sized to its content
 * in `ch`, which is exact for the mono face, so the suffix sits right after it.
 */
const props = defineProps<{
  modelValue: string;
  suffix: string;
  placeholder?: string;
  id?: string;
  ariaLabel?: string;
  invalid?: boolean;
}>();

const emit = defineEmits<{ "update:modelValue": [string] }>();

const el = ref<HTMLInputElement | null>(null);

const width = computed(
  () => `${Math.max((props.modelValue || props.placeholder || "").length, 1) + 0.5}ch`,
);

defineExpose({ focus: () => el.value?.focus() });
</script>

<template>
  <div
    :class="
      cn(
        'flex h-10 cursor-text items-center overflow-hidden rounded-lg border bg-background px-3 font-mono text-sm transition-colors focus-within:border-brand focus-within:ring-1 focus-within:ring-brand',
        invalid && 'border-destructive focus-within:border-destructive focus-within:ring-destructive',
      )
    "
    @click="el?.focus()"
  >
    <input
      :id="id"
      ref="el"
      :value="modelValue"
      :placeholder="placeholder"
      :aria-label="ariaLabel"
      :style="{ width }"
      class="min-w-0 max-w-full bg-transparent outline-none placeholder:text-faint"
      spellcheck="false"
      autocomplete="off"
      @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
    />
    <span class="shrink-0 text-faint">{{ suffix }}</span>
  </div>
</template>
