<script setup lang="ts">
import { RadioGroupItem, RadioGroupRoot } from "reka-ui";

import { cn } from "@/lib/utils";

/**
 * The draft's segmented picker: an inset well of mono options with one filled.
 * Built on reka-ui's RadioGroup, so arrow keys and radio roles come for free.
 * `brand` fills the selected option blue (PHP, Node); `plain` lifts it onto the
 * card surface (Authentication, Testing, Frontend dependencies). `size="md"`
 * renders taller sans-serif options (Settings: theme, update channel, log source).
 */
withDefaults(
  defineProps<{
    modelValue: string;
    options: readonly { value: string; label: string }[];
    disabled?: boolean;
    ariaLabel?: string;
    variant?: "brand" | "plain";
    size?: "sm" | "md";
  }>(),
  { variant: "brand", disabled: false, size: "sm" },
);

const emit = defineEmits<{ "update:modelValue": [string] }>();

function onUpdate(value: unknown): void {
  if (typeof value === "string") emit("update:modelValue", value);
}
</script>

<template>
  <RadioGroupRoot
    :model-value="modelValue"
    :disabled="disabled"
    :aria-label="ariaLabel"
    orientation="horizontal"
    :class="
      cn(
        'inline-flex shrink-0 items-center gap-0.5 border bg-muted data-[disabled]:opacity-50',
        size === 'md' ? 'rounded-lg p-[3px]' : 'rounded-[7px] p-0.5',
      )
    "
    @update:model-value="onUpdate"
  >
    <RadioGroupItem
      v-for="o in options"
      :key="o.value"
      :value="o.value"
      :class="
        cn(
          'min-w-[2.25rem] rounded-[5px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed',
          size === 'md' ? 'h-7 px-3 font-sans text-[12.5px] font-medium' : 'h-6 px-2.5 font-mono text-xs',
          o.value === modelValue
            ? variant === 'brand'
              ? 'bg-brand font-semibold text-white'
              : 'bg-card font-semibold text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground',
        )
      "
    >
      {{ o.label }}
    </RadioGroupItem>
  </RadioGroupRoot>
</template>
