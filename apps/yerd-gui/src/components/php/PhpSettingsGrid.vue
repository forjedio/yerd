<script setup lang="ts">
import { computed } from "vue";

import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** One row of the settings grid. */
export interface SettingsGridRow {
  key: string;
  label: string;
  hint: string;
  /** `text` is a mono field, `toggle` a segmented well, `ffi` the caller's `ffi` slot. */
  kind: "text" | "toggle" | "ffi";
  value: string;
  placeholder?: string;
  unit?: "sec";
  options?: readonly { value: string; label: string }[];
  /** Whether the value differs from what the row inherits (a blue dot when `markers`). */
  changed?: boolean;
  problem?: string | null;
}

/**
 * The draft's two-column settings card, shared by the Shared defaults pane and
 * each version's inspector: 40px rows with a label (its hint in a tooltip) and a
 * 120px control at the right. With `markers`, changed rows get a blue dot and an
 * ink label. Edits go up as `update(key, value)`; the caller owns the values.
 */
const props = defineProps<{
  rows: SettingsGridRow[];
  markers: boolean;
  disabled: boolean;
  /** Prefix for the text fields' ids, e.g. `set-8.4`. */
  idPrefix: string;
  /** Appended to the toggle rows' accessible names, e.g. ` for PHP 8.4`. */
  ariaSuffix?: string;
}>();

const emit = defineEmits<{ update: [key: string, value: string] }>();

const columns = computed(() => {
  const half = Math.ceil(props.rows.length / 2);
  return [props.rows.slice(0, half), props.rows.slice(half)];
});

function fieldId(key: string): string {
  return `${props.idPrefix}-${key}`;
}
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <div class="grid grid-cols-2 divide-x rounded-[10px] border bg-card" data-testid="settings-grid">
      <div v-for="(col, ci) in columns" :key="ci" class="min-w-0 py-1.5">
        <div
          v-for="row in col"
          :key="row.key"
          class="px-3.5"
          :data-row="row.key"
          :data-changed="markers && row.changed ? 'true' : undefined"
        >
          <div class="flex min-h-10 items-center gap-2">
            <span
              v-if="markers"
              :class="cn('size-1.5 shrink-0 rounded-full', row.changed ? 'bg-brand' : 'bg-transparent')"
              aria-hidden="true"
            />
            <Tooltip>
              <TooltipTrigger as-child>
                <label
                  :for="row.kind === 'text' ? fieldId(row.key) : undefined"
                  :class="
                    cn(
                      'min-w-0 flex-1 cursor-help truncate text-[13px]',
                      !markers || row.changed ? 'text-foreground' : 'text-muted-foreground',
                    )
                  "
                >
                  {{ row.label }}
                </label>
              </TooltipTrigger>
              <TooltipContent side="top" class="max-w-xs">{{ row.hint }}</TooltipContent>
            </Tooltip>

            <span v-if="row.kind === 'text'" class="relative w-[120px] shrink-0">
              <input
                :id="fieldId(row.key)"
                :value="row.value"
                :placeholder="row.placeholder"
                :disabled="disabled"
                :aria-invalid="row.problem ? 'true' : undefined"
                :class="
                  cn(
                    'h-7 w-full rounded-md border bg-background px-2 font-mono text-[13px] placeholder:text-faint focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
                    row.unit && 'pr-9',
                    row.problem && 'border-destructive',
                  )
                "
                @input="emit('update', row.key, ($event.target as HTMLInputElement).value)"
              />
              <span
                v-if="row.unit"
                class="pointer-events-none absolute inset-y-0 right-2 flex items-center text-[11px] text-faint"
                aria-hidden="true"
              >
                {{ row.unit }}
              </span>
            </span>

            <SegmentedControl
              v-else-if="row.kind === 'toggle'"
              variant="plain"
              :model-value="row.value"
              :options="row.options ?? []"
              :disabled="disabled"
              :aria-label="`${row.label}${ariaSuffix ?? ''}`"
              @update:model-value="(v: string) => emit('update', row.key, v)"
            />

            <span v-else class="flex shrink-0 justify-end">
              <slot name="ffi" />
            </span>
          </div>
          <p v-if="row.problem" class="-mt-1 pb-1.5 text-right text-[11px] text-destructive">
            {{ row.problem }}
          </p>
        </div>
      </div>
    </div>
  </TooltipProvider>
</template>
