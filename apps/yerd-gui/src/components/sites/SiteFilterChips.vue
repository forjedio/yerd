<script setup lang="ts">
import { Check, ChevronDown, Search, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ChipKey, SiteChip } from "@/lib/sites";
import { cn } from "@/lib/utils";

/**
 * The list's filter row: one pill per chip with its count, and a compact text
 * filter on the right (focused by ⌘F through `focus()`).
 *
 * When the pills would wrap onto more than {@link MAX_ROWS} rows, they collapse
 * into a single dropdown showing the active chip. An invisible copy of the pill
 * row, laid out at the same width, is measured to decide this, so the choice
 * follows the window width and the number of chips.
 */
const props = defineProps<{
  chips: SiteChip[];
  modelValue: ChipKey;
  query: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [ChipKey];
  "update:query": [string];
}>();

const MAX_ROWS = 2;

const input = ref<HTMLInputElement | null>(null);
const measure = ref<HTMLElement | null>(null);
const rows = ref(1);
const collapsed = computed(() => rows.value > MAX_ROWS);
const active = computed(() => props.chips.find((c) => c.key === props.modelValue) ?? props.chips[0]);

function countRows(): void {
  const el = measure.value;
  if (!el) return;
  const tops = new Set<number>();
  for (const child of Array.from(el.children)) tops.add((child as HTMLElement).offsetTop);
  rows.value = Math.max(tops.size, 1);
}

let observer: ResizeObserver | null = null;

onMounted(() => {
  countRows();
  if (typeof ResizeObserver === "undefined" || !measure.value) return;
  observer = new ResizeObserver(() => countRows());
  observer.observe(measure.value);
});

onBeforeUnmount(() => observer?.disconnect());

watch(
  () => props.chips.map((c) => `${c.key}:${c.count}`).join("|"),
  () => void nextTick(countRows),
);

defineExpose({ focus: () => input.value?.focus() });

function pillClass(key: ChipKey): string {
  return cn(
    "flex h-7 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
    props.modelValue === key
      ? "border-brand/60 bg-brand/15 font-semibold text-foreground"
      : "text-muted-foreground hover:text-foreground",
  );
}
</script>

<template>
  <div class="flex items-start gap-2">
    <div class="relative min-w-0 flex-1">
      <div
        ref="measure"
        class="pointer-events-none invisible absolute inset-x-0 top-0 flex flex-wrap items-center gap-1.5"
        aria-hidden="true"
        data-testid="chips-measure"
      >
        <span v-for="c in chips" :key="c.key" :class="pillClass(c.key)">
          {{ c.label }} <span class="font-mono text-[11px]">{{ c.count }}</span>
        </span>
      </div>

      <DropdownMenu v-if="collapsed && active">
        <DropdownMenuTrigger as-child>
          <button
            type="button"
            :class="cn(pillClass(active.key), 'border-brand/60 bg-brand/15')"
            aria-label="Filter sites"
            data-testid="chips-dropdown"
          >
            {{ active.label }}
            <span class="font-mono text-[11px] text-brand">{{ active.count }}</span>
            <ChevronDown class="size-3.5 text-muted-foreground" aria-hidden="true" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-56">
          <DropdownMenuItem
            v-for="c in chips"
            :key="c.key"
            :data-chip="c.key"
            @select="emit('update:modelValue', c.key)"
          >
            <Check
              :class="cn('size-4', modelValue === c.key ? 'opacity-100' : 'opacity-0')"
              aria-hidden="true"
            />
            <span class="flex-1">{{ c.label }}</span>
            <span class="font-mono text-[11px] text-faint">{{ c.count }}</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <div v-else class="flex flex-wrap items-center gap-1.5" role="group" aria-label="Filter sites">
        <button
          v-for="c in chips"
          :key="c.key"
          type="button"
          :data-chip="c.key"
          :aria-pressed="modelValue === c.key"
          :class="pillClass(c.key)"
          @click="emit('update:modelValue', c.key)"
        >
          {{ c.label }}
          <span
            :class="cn('font-mono text-[11px]', modelValue === c.key ? 'text-brand' : 'text-faint')"
          >{{ c.count }}</span>
        </button>
      </div>
    </div>
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
