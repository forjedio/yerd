<script setup lang="ts">
import { Check, ChevronsUpDown, LayoutGrid, Search } from "lucide-vue-next";
import {
  ComboboxAnchor,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxLabel,
  ComboboxPortal,
  ComboboxRoot,
  ComboboxTrigger,
  ComboboxViewport,
} from "reka-ui";
import { computed } from "vue";

import { FRAMEWORKS, type StarterId } from "@/composables/useSiteStarters";
import type { ComposerFramework } from "@/ipc/types";
import { cn } from "@/lib/utils";

/**
 * The "Another starter" tile: a searchable list of the starters without a tile
 * of their own (CodeIgniter, CakePHP). Once one is picked the tile becomes the
 * selected card, showing its package.
 */
const props = defineProps<{ modelValue: StarterId }>();
const emit = defineEmits<{ "update:modelValue": [StarterId] }>();

/** Starters listed here rather than as tiles. */
const PICKER_STARTERS: ComposerFramework[] = ["codeigniter", "cakephp"];

const picked = computed(() =>
  (PICKER_STARTERS as string[]).includes(props.modelValue)
    ? (props.modelValue as ComposerFramework)
    : null,
);

function onPick(value: unknown): void {
  if (typeof value === "string" && (PICKER_STARTERS as string[]).includes(value)) {
    emit("update:modelValue", value as ComposerFramework);
  }
}
</script>

<template>
  <ComboboxRoot :model-value="picked ?? undefined" :ignore-filter="false" @update:model-value="onPick">
    <ComboboxAnchor class="w-full">
      <ComboboxTrigger
        data-testid="starter-picker"
        :class="
          cn(
            'flex h-11 w-full items-center gap-2.5 rounded-[10px] border px-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            picked ? 'border-brand bg-brand/10' : 'bg-background hover:border-brand/40',
          )
        "
      >
        <LayoutGrid class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <template v-if="picked">
          <span class="text-[13px] font-semibold">{{ FRAMEWORKS[picked].label }}</span>
          <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
            {{ FRAMEWORKS[picked].pkg }}
          </span>
        </template>
        <template v-else>
          <span class="text-[13px] font-semibold">Another starter</span>
          <span class="min-w-0 flex-1 truncate text-xs text-faint">CodeIgniter, CakePHP…</span>
        </template>
        <ChevronsUpDown class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
      </ComboboxTrigger>
    </ComboboxAnchor>

    <ComboboxPortal>
      <ComboboxContent
        position="popper"
        :side-offset="6"
        class="z-50 w-[var(--reka-combobox-trigger-width)] min-w-[18rem] overflow-hidden rounded-[10px] border bg-popover text-popover-foreground shadow-md"
      >
        <div class="flex items-center gap-2 border-b px-3">
          <Search class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <ComboboxInput
            auto-focus
            placeholder="Search starters"
            class="h-10 w-full bg-transparent text-sm outline-none placeholder:text-faint"
          />
        </div>
        <ComboboxEmpty class="px-3 py-5 text-center text-sm text-muted-foreground">
          No matching starter.
        </ComboboxEmpty>
        <ComboboxViewport class="p-1.5">
          <ComboboxGroup>
            <ComboboxLabel class="px-2 pb-1 pt-1.5 text-[10px] font-semibold uppercase tracking-[0.6px] text-faint">
              Frameworks
            </ComboboxLabel>
            <ComboboxItem
              v-for="id in PICKER_STARTERS"
              :key="id"
              :value="id"
              :text-value="FRAMEWORKS[id].label"
              class="flex cursor-pointer select-none items-center gap-2 rounded-md px-2 py-2 text-[13px] outline-none data-[highlighted]:bg-brand/15"
            >
              <Check :class="cn('size-3.5 shrink-0 text-brand', picked === id ? '' : 'opacity-0')" />
              <span class="font-medium">{{ FRAMEWORKS[id].label }}</span>
              <span class="ml-auto truncate pl-3 font-mono text-[11px] text-faint">
                {{ FRAMEWORKS[id].pkg }}
              </span>
            </ComboboxItem>
          </ComboboxGroup>
        </ComboboxViewport>
        <p class="border-t px-3 py-2 text-[11px] text-faint">↑↓ move · Return pick · Esc close</p>
      </ComboboxContent>
    </ComboboxPortal>
  </ComboboxRoot>
</template>
