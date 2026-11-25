<script setup lang="ts">
import { computed } from "vue";
import { ChevronRight, Info, KeyRound } from "lucide-vue-next";

import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import { PROVIDER_ORDER, providerName } from "@/lib/providers";
import { filterShares, kindLabel, type ShareFilter, type ShareItem, stateDotClass, stateLabel } from "@/lib/share";
import { cn } from "@/lib/utils";

import ProviderTile from "./providers/ProviderTile.vue";

/**
 * The Share list: a `SHARES · n` header with an All / Cloudflare / ngrok /
 * zrok filter, then one flat row per share. `list` is the column beside the
 * inspector (wide windows): the provider tile, the site with its provider and
 * kind, and a status dot with the public host. `table` spans the window
 * (compact windows) with SITE, PROVIDER, PUBLIC URL and STATUS columns; a click
 * opens the inspector drawer and there is no persistent selection.
 */
const props = defineProps<{
  layout: "list" | "table";
  items: ShareItem[];
  filter: ShareFilter;
  selected: string | null;
}>();

const emit = defineEmits<{ select: [key: string]; "update:filter": [filter: ShareFilter] }>();

const FILTERS: readonly { value: ShareFilter; label: string }[] = [
  { value: "all", label: "All" },
  ...PROVIDER_ORDER.map((p) => ({ value: p, label: providerName(p) })),
];

const shown = computed(() => filterShares(props.items, props.filter));

function hostText(item: ShareItem): string {
  return item.host ?? "Waiting for URL…";
}

function providerKind(item: ShareItem): string {
  return `${providerName(item.provider)} · ${kindLabel(item)}`;
}

function onFilter(value: string): void {
  const next = FILTERS.find((f) => f.value === value);
  if (next) emit("update:filter", next.value);
}

const TABLE_COLS = "grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.6fr)_140px_20px] items-center gap-x-3";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="share-list" :data-layout="layout">
    <div class="flex items-center justify-between gap-2 pb-2 pr-4 pt-4">
      <span :class="SECTION_LABEL" data-testid="shares-count">Shares · {{ items.length }}</span>
      <SegmentedControl
        :model-value="filter"
        :options="FILTERS"
        variant="plain"
        aria-label="Filter shares by provider"
        data-testid="share-filter"
        @update:model-value="onFilter"
      />
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <div
        v-if="layout === 'table' && shown.length"
        :class="cn(TABLE_COLS, 'px-2.5 pb-1 pt-1 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
        aria-hidden="true"
      >
        <span>Site</span><span>Provider</span><span>Public URL</span><span>Status</span><span />
      </div>

      <p v-if="!shown.length" class="px-2.5 py-6 text-center text-[13px] text-faint" data-testid="share-filter-empty">
        No {{ filter === "all" ? "" : `${providerName(filter)} ` }}shares.
      </p>

      <div v-else-if="layout === 'list'" class="flex flex-col gap-1">
        <button
          v-for="item in shown"
          :key="item.key"
          type="button"
          :data-key="item.key"
          :aria-current="selected === item.key ? 'true' : undefined"
          :class="
            cn(
              'flex h-[58px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              selected === item.key ? 'border-brand/45 bg-brand/[0.14]' : 'border-transparent hover:bg-accent/60',
            )
          "
          @click="emit('select', item.key)"
        >
          <ProviderTile :provider="item.provider" size="md" />
          <span class="min-w-0 flex-1">
            <span class="flex items-center gap-1.5">
              <span class="truncate text-[13px] font-semibold">{{ item.site }}</span>
              <span class="truncate text-[11px] text-faint" data-testid="share-kind">{{ providerKind(item) }}</span>
              <KeyRound
                v-if="item.protected"
                class="size-3 shrink-0 text-muted-foreground"
                aria-label="Password protected"
              />
            </span>
            <span class="mt-1 flex items-center gap-1.5 text-xs">
              <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(item.state))" aria-hidden="true" />
              <span :class="cn('truncate', item.host ? 'font-mono text-muted-foreground' : 'text-faint')">
                {{ hostText(item) }}
              </span>
            </span>
          </span>
        </button>
      </div>

      <template v-else>
        <button
          v-for="item in shown"
          :key="item.key"
          type="button"
          :data-key="item.key"
          :class="
            cn(
              TABLE_COLS,
              'h-[50px] w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
            )
          "
          @click="emit('select', item.key)"
        >
          <span class="flex min-w-0 items-center gap-2">
            <ProviderTile :provider="item.provider" />
            <span class="truncate text-[13px] font-semibold">{{ item.site }}</span>
            <KeyRound
              v-if="item.protected"
              class="size-3 shrink-0 text-muted-foreground"
              aria-label="Password protected"
            />
          </span>
          <span class="truncate text-xs text-muted-foreground" data-testid="share-provider">{{ providerKind(item) }}</span>
          <span :class="cn('truncate text-xs', item.host ? 'font-mono text-muted-foreground' : 'text-faint')">
            {{ hostText(item) }}
          </span>
          <span class="flex items-center gap-1.5 text-xs">
            <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(item.state))" aria-hidden="true" />
            <span :class="item.state === 'failed' ? 'text-destructive' : 'text-foreground'">{{ stateLabel(item.state) }}</span>
          </span>
          <ChevronRight class="size-4 text-faint" aria-hidden="true" />
        </button>
      </template>

      <p class="mt-3 flex items-center gap-1.5 px-2.5 text-[11px] text-faint" data-testid="share-hint">
        <Info class="size-3 shrink-0" aria-hidden="true" />
        A site can be shared through several providers at once.
      </p>
    </div>
  </div>
</template>
