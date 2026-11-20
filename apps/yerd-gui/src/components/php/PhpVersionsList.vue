<script setup lang="ts">
import { computed } from "vue";
import { ChevronRight, Plus, SlidersHorizontal, Star } from "lucide-vue-next";

import Spinner from "@/components/ui/Spinner.vue";
import { LIST_LABEL } from "@/components/ui/styles";
import type { PhpExtInfo, PhpPoolStatus, PhpUpdate, PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import { bannerCopy, configuredCount, statusLine, updateLabel, xdebugCell } from "@/lib/php";
import { isLegacyVersion } from "@/lib/phpVersion";
import { CHIP_TONE, cn, humaniseBytes, plural, poolStateLabel, runStateDotClass } from "@/lib/utils";

/** What the list can select: the Shared defaults pane or a version. */
export type PhpSelection = "shared" | PhpVersion;

/**
 * The PHP version list. `list` is the column beside the inspector (wide
 * windows): two-line rows with chips, the pool status and an update chip or a
 * "customised" tag. `table` spans the window (compact windows) with PATCH, FPM,
 * MEMORY, XDEBUG and UPDATE columns, and has no persistent selection. Both open
 * with the Shared defaults row and end with "Install another version"; the
 * update banner sits pinned below.
 */
const props = defineProps<{
  layout: "list" | "table";
  versions: PhpVersion[];
  installed: PhpVersion[];
  defaultVersion: PhpVersion | null;
  selected: PhpSelection | null;
  data: PhpVersionsResponse | null;
  extensions: Record<PhpVersion, PhpExtInfo[]> | null;
  pools: Record<PhpVersion, PhpPoolStatus>;
  updates: PhpUpdate[];
  dirty: Record<string, boolean>;
  /** The view's in-flight action keys, e.g. `restart:8.4` or `update:all`. */
  busy: string[];
}>();

const emit = defineEmits<{
  select: [key: PhpSelection];
  install: [];
  update: [version: PhpVersion | null];
}>();

const installedRows = computed(() => props.versions.filter((v) => props.installed.includes(v)));
const orphanRows = computed(() => props.versions.filter((v) => !props.installed.includes(v)));
const updateFor = computed<Record<string, PhpUpdate>>(() =>
  Object.fromEntries(props.updates.map((u) => [u.version, u])),
);
const banner = computed(() => bannerCopy(props.updates));
const bannerBusy = computed(() =>
  banner.value ? props.busy.some((k) => k.startsWith("update:")) : false,
);

function count(v: PhpVersion): number {
  return configuredCount(props.data, props.extensions, v, props.installed);
}

function rowBusy(v: PhpVersion): boolean {
  return props.busy.some((k) => k.endsWith(`:${v}`));
}

function state(v: PhpVersion) {
  return props.pools[v]?.state;
}

function statusClass(v: PhpVersion): string {
  const s = state(v);
  if (s === "running") return "text-muted-foreground";
  if (s === "failed") return "text-destructive";
  return "text-faint";
}

function update(v: PhpVersion) {
  return updateLabel(updateFor.value[v], isLegacyVersion(v));
}

const CHIP = "inline-flex h-[18px] shrink-0 items-center gap-1 rounded px-1.5 text-[11px] font-medium";
const UPDATE_CHIP =
  "inline-flex h-5 shrink-0 items-center rounded border border-warning/40 bg-warning/15 px-1.5 font-mono text-[11px] font-medium text-warning";
const TABLE_COLS =
  "grid grid-cols-[minmax(0,1fr)_90px_110px_90px_100px_110px_20px] items-center gap-x-2";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="php-list" :data-layout="layout">
    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <button
        type="button"
        data-key="shared"
        :aria-current="selected === 'shared' ? 'true' : undefined"
        :class="
          cn(
            'flex h-[54px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none',
            layout === 'list' && selected === 'shared'
              ? 'border-brand/50 bg-card'
              : 'border-transparent hover:bg-accent/60',
            layout === 'table' ? 'focus-visible:bg-brand/25' : 'focus-visible:ring-1 focus-visible:ring-ring',
          )
        "
        @click="emit('select', 'shared')"
      >
        <span class="flex size-[30px] shrink-0 items-center justify-center rounded-md border bg-card text-muted-foreground">
          <SlidersHorizontal class="size-4" aria-hidden="true" />
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[13px] font-semibold">Shared defaults</span>
          <span class="block truncate text-xs text-faint">Settings every version starts from</span>
        </span>
        <span v-if="dirty.shared" class="flex items-center">
          <span class="size-1.5 rounded-full bg-brand" aria-hidden="true" />
          <span class="sr-only">unsaved changes</span>
        </span>
        <ChevronRight v-if="layout === 'table'" class="size-4 shrink-0 text-faint" aria-hidden="true" />
      </button>

      <template v-if="layout === 'list'">
        <template
          v-for="group in [
            { key: 'installed', label: 'Installed', rows: installedRows },
            { key: 'orphans', label: 'Not installed', rows: orphanRows },
          ]"
          :key="group.key"
        >
          <template v-if="group.rows.length">
            <p :class="LIST_LABEL">{{ group.label }}</p>
            <div class="flex flex-col gap-1">
              <button
                v-for="v in group.rows"
                :key="v"
                type="button"
                :data-key="v"
                :aria-current="selected === v ? 'true' : undefined"
                :class="
                  cn(
                    'flex h-[58px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                    selected === v ? 'border-brand/50 bg-card' : 'border-transparent hover:bg-accent/60',
                  )
                "
                @click="emit('select', v)"
              >
                <span class="min-w-0 flex-1">
                  <span class="flex items-center gap-2">
                    <span class="font-mono text-sm font-bold">PHP {{ v }}</span>
                    <span v-if="v === defaultVersion" :class="cn(CHIP, CHIP_TONE.brand)" data-testid="chip-default">
                      <Star class="size-3" aria-hidden="true" /> Default
                    </span>
                    <span v-if="isLegacyVersion(v)" :class="cn(CHIP, CHIP_TONE.warning)" data-testid="chip-legacy">
                      Legacy
                    </span>
                  </span>
                  <span class="mt-1 flex items-center gap-1.5 text-xs" data-testid="row-status">
                    <template v-if="installed.includes(v)">
                      <span :class="cn('size-1.5 shrink-0 rounded-full', runStateDotClass(state(v)))" aria-hidden="true" />
                      <span :class="cn('truncate', statusClass(v))">
                        {{ statusLine(pools[v], "row") }}<template v-if="data?.xdebug?.[v]?.enabled"> · Xdebug on</template>
                      </span>
                    </template>
                    <span v-else class="truncate text-faint">
                      not installed · {{ plural(count(v), "extension", "extensions") }}
                    </span>
                  </span>
                </span>
                <span v-if="dirty[v]" class="flex items-center">
                  <span class="size-1.5 rounded-full bg-brand" aria-hidden="true" />
                  <span class="sr-only">unsaved changes</span>
                </span>
                <Spinner v-if="rowBusy(v)" class="size-3.5 shrink-0" />
                <span v-if="update(v).kind === 'update'" :class="UPDATE_CHIP" data-testid="chip-update">
                  {{ update(v).text }}
                </span>
                <span
                  v-else-if="installed.includes(v) && count(v) > 0"
                  class="shrink-0 text-[11px] text-faint"
                  :title="plural(count(v), 'configured item', 'configured items')"
                  data-testid="row-customised"
                >
                  customised
                </span>
              </button>
            </div>
          </template>
        </template>
      </template>

      <template v-else>
        <div
          :class="cn(TABLE_COLS, 'mt-2 px-2.5 pb-2 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
          aria-hidden="true"
        >
          <span>Installed</span><span>Patch</span><span>FPM</span><span>Memory</span><span>Xdebug</span><span>Update</span><span />
        </div>
        <button
          v-for="v in versions"
          :key="v"
          type="button"
          :data-key="v"
          :aria-label="`PHP ${v}`"
          :class="
            cn(
              TABLE_COLS,
              'h-12 w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
            )
          "
          @click="emit('select', v)"
        >
          <span class="flex min-w-0 items-center gap-2">
            <span class="truncate font-mono text-sm font-bold">PHP {{ v }}</span>
            <span v-if="v === defaultVersion" :class="cn(CHIP, CHIP_TONE.brand)" data-testid="chip-default">
              <Star class="size-3" aria-hidden="true" /> Default
            </span>
            <span v-if="isLegacyVersion(v)" :class="cn(CHIP, CHIP_TONE.warning)" data-testid="chip-legacy">
              Legacy
            </span>
            <span v-if="dirty[v]" class="flex items-center">
              <span class="size-1.5 rounded-full bg-brand" aria-hidden="true" />
              <span class="sr-only">unsaved changes</span>
            </span>
            <Spinner v-if="rowBusy(v)" class="size-3.5 shrink-0" />
          </span>
          <template v-if="installed.includes(v)">
            <span class="truncate font-mono text-xs text-muted-foreground" data-testid="cell-patch">
              {{ pools[v]?.installed_patch ?? "-" }}
            </span>
            <span class="flex items-center gap-1.5 text-xs" data-testid="cell-fpm">
              <span :class="cn('size-1.5 shrink-0 rounded-full', runStateDotClass(state(v)))" aria-hidden="true" />
              <span :class="statusClass(v)">{{ poolStateLabel(state(v)) }}</span>
            </span>
            <span class="truncate font-mono text-xs text-muted-foreground" data-testid="cell-memory">
              {{ humaniseBytes(pools[v]?.rss_bytes) }}
            </span>
            <span class="text-xs" data-testid="cell-xdebug">
              <span
                v-if="xdebugCell(v, data) === 'on'"
                class="inline-flex h-5 items-center rounded bg-success/15 px-1.5 text-[11px] font-medium text-success"
              >
                Xdebug on
              </span>
              <span v-else class="text-faint">{{ xdebugCell(v, data) }}</span>
            </span>
            <span class="text-xs" data-testid="cell-update">
              <span v-if="update(v).kind === 'update'" :class="UPDATE_CHIP">{{ update(v).text }}</span>
              <span v-else class="text-faint">{{ update(v).text }}</span>
            </span>
          </template>
          <template v-else>
            <span class="text-xs text-faint">-</span>
            <span class="text-xs text-faint" data-testid="cell-fpm">not installed</span>
            <span class="text-xs text-faint">-</span>
            <span class="text-xs text-faint">-</span>
            <span class="text-xs text-faint">-</span>
          </template>
          <ChevronRight class="size-4 text-faint" aria-hidden="true" />
        </button>
      </template>

      <button
        type="button"
        class="mt-2 flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-dashed text-[13px] text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        data-testid="install-row"
        @click="emit('install')"
      >
        <Plus class="size-4" aria-hidden="true" /> Install another version
      </button>
    </div>

    <div v-if="banner" class="shrink-0 pb-4 pr-4 pt-1">
      <div
        class="flex min-h-16 items-center gap-3 rounded-[10px] border border-warning/40 bg-warning/10 px-3.5 py-2.5"
        data-testid="update-banner"
      >
        <div class="min-w-0 flex-1">
          <p class="text-[13px] font-semibold">{{ banner.title }}</p>
          <p class="truncate text-xs text-muted-foreground">{{ banner.detail }}</p>
        </div>
        <button
          type="button"
          class="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-warning px-3 text-xs font-semibold text-black transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
          :disabled="bannerBusy"
          data-testid="banner-update"
          @click="emit('update', banner.target)"
        >
          <Spinner v-if="bannerBusy" class="size-3.5" />
          {{ banner.action }}
        </button>
      </div>
    </div>
  </div>
</template>
