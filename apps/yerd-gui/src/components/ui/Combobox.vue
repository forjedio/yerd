<script setup lang="ts" generic="T extends string">
import {
  ComboboxAnchor,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxPortal,
  ComboboxRoot,
  ComboboxTrigger,
  ComboboxViewport,
} from "reka-ui";
import { Check, ChevronsUpDown, Search } from "lucide-vue-next";

import { cn } from "@/lib/utils";

/** One selectable row. `sublabel` renders muted to the right (e.g. a doc-root). */
export type ComboboxOption<V extends string> = {
  value: V;
  label: string;
  sublabel?: string;
};

const props = withDefaults(
  defineProps<{
    modelValue: T | null;
    options: readonly ComboboxOption<T>[];
    placeholder?: string;
    searchPlaceholder?: string;
    emptyText?: string;
    disabled?: boolean;
    ariaLabel?: string;
  }>(),
  {
    placeholder: "Select…",
    searchPlaceholder: "Search…",
    emptyText: "No matches.",
    disabled: false,
    ariaLabel: undefined,
  },
);

const emit = defineEmits<{ "update:modelValue": [T] }>();

function labelFor(value: T | null): string {
  return props.options.find((o) => o.value === value)?.label ?? "";
}
</script>

<template>
  <ComboboxRoot
    :model-value="modelValue ?? undefined"
    :disabled="disabled"
    :ignore-filter="false"
    @update:model-value="(v) => emit('update:modelValue', v as T)"
  >
    <ComboboxAnchor class="w-full">
      <ComboboxTrigger
        :aria-label="ariaLabel"
        :class="
          cn(
            'flex h-9 w-full items-center justify-between gap-2 rounded-md border border-input bg-background px-3 text-sm shadow-sm transition-colors',
            'hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            'disabled:cursor-not-allowed disabled:opacity-50 data-[state=open]:ring-1 data-[state=open]:ring-ring',
          )
        "
      >
        <span :class="cn('truncate', modelValue ? 'text-foreground' : 'text-muted-foreground')">
          {{ modelValue ? labelFor(modelValue) : placeholder }}
        </span>
        <ChevronsUpDown class="size-4 shrink-0 opacity-50" />
      </ComboboxTrigger>
    </ComboboxAnchor>

    <ComboboxPortal>
      <ComboboxContent
        position="popper"
        :side-offset="6"
        :class="
          cn(
            'z-50 w-[var(--reka-combobox-trigger-width)] min-w-[16rem] overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-md',
            'animate-in fade-in-0 zoom-in-95 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95',
          )
        "
      >
        <div class="flex items-center gap-2 border-b px-3">
          <Search class="size-4 shrink-0 text-muted-foreground" />
          <ComboboxInput
            auto-focus
            :placeholder="searchPlaceholder"
            class="h-9 w-full bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>

        <ComboboxEmpty class="px-3 py-6 text-center text-sm text-muted-foreground">
          {{ emptyText }}
        </ComboboxEmpty>

        <ComboboxViewport class="max-h-64 overflow-y-auto p-1">
          <ComboboxItem
            v-for="o in options"
            :key="o.value"
            :value="o.value"
            :text-value="o.label"
            :class="
              cn(
                'relative flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none transition-colors',
                'data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground',
                'data-[state=checked]:font-medium',
              )
            "
          >
            <Check
              :class="
                cn('size-4 shrink-0', o.value === modelValue ? 'opacity-100 text-brand' : 'opacity-0')
              "
            />
            <span class="truncate">{{ o.label }}</span>
            <span v-if="o.sublabel" class="ml-auto truncate pl-2 text-xs text-muted-foreground">
              {{ o.sublabel }}
            </span>
          </ComboboxItem>
        </ComboboxViewport>
      </ComboboxContent>
    </ComboboxPortal>
  </ComboboxRoot>
</template>
