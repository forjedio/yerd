<script setup lang="ts">
import { RadioGroupItem, RadioGroupRoot } from "reka-ui";

import { cn } from "@/lib/utils";

/**
 * The draft's segmented picker: an inset well of mono options with one filled.
 * Built on reka-ui's RadioGroup, so arrow keys and radio roles come for free.
 * `brand` fills the selected option blue (PHP, Node); `plain` lifts it onto the
 * card surface (Authentication, Testing, Frontend dependencies).
 */
withDefaults(
  defineProps<{
    modelValue: string;
    options: readonly { value: string; label: string }[];
    disabled?: boolean;
    ariaLabel?: string;
    variant?: "brand" | "plain";
  }>(),
  { variant: "brand", disabled: false },
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
    class="inline-flex shrink-0 items-center gap-0.5 rounded-[7px] border bg-muted p-0.5 data-[disabled]:opacity-50"
    @update:model-value="onUpdate"
  >
    <RadioGroupItem
      v-for="o in options"
      :key="o.value"
      :value="o.value"
      :class="
        cn(
          'h-6 min-w-[2.25rem] rounded-[5px] px-2.5 font-mono text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed',
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
