<script setup lang="ts">
import { Search, X } from "lucide-vue-next";
import { ref } from "vue";

import FilterChips from "@/components/ui/FilterChips.vue";
import type { ChipKey, SiteChip } from "@/lib/sites";

/**
 * The list's filter row: one pill per chip with its count (collapsing into a
 * dropdown past two rows, see `FilterChips`), and a compact text filter on the
 * right (focused by ⌘F through `focus()`).
 */
defineProps<{
  chips: SiteChip[];
  modelValue: ChipKey;
  query: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [ChipKey];
  "update:query": [string];
}>();

const input = ref<HTMLInputElement | null>(null);

defineExpose({ focus: () => input.value?.focus() });
</script>

<template>
  <div class="flex items-start gap-2">
    <FilterChips
      class="flex-1"
      :chips="chips"
      :model-value="modelValue"
      label="Filter sites"
      @update:model-value="(k) => emit('update:modelValue', k)"
    />
    <div
      class="flex h-7 w-44 shrink-0 items-center gap-1.5 rounded-full border bg-background px-2.5 focus-within:border-brand/60"
    >
      <Search class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
      <input
        ref="input"
        :value="query"
        type="text"
        placeholder="Filter sites…"
        aria-label="Filter sites"
        spellcheck="false"
        class="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-faint"
        @input="emit('update:query', ($event.target as HTMLInputElement).value)"
        @keydown.esc="emit('update:query', '')"
      />
      <button
        v-if="query"
        type="button"
        class="shrink-0 text-faint hover:text-foreground"
        aria-label="Clear filter"
        @click="emit('update:query', '')"
      >
        <X class="size-3.5" />
      </button>
    </div>
  </div>
</template>
