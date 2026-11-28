<script setup lang="ts">
import { Check, ChevronDown, Globe, Pause, Play, SlidersHorizontal, Trash2 } from "lucide-vue-next";
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { InsightsBusy } from "@/composables/useInsights";
import type { InsightsRange, SiteEntry } from "@/ipc/types";
import { isRange, RANGE_OPTIONS } from "@/lib/insights";
import { frameworkLabel } from "@/lib/sites";
import { cn } from "@/lib/utils";

/**
 * The Insights toolbar, its own bar under the page header: the site picker
 * (recording sites marked), the "Recording · this site" pill that turns
 * recording on and off, the 5m / 1h / 24h range, the view-only pause, clear
 * and the settings dialog, with the range and buttons pushed to the right.
 * In the compact layout the pill and picker shrink to their essentials.
 */
const props = defineProps<{
  sites: readonly SiteEntry[];
  site: string | null;
  tld: string;
  /** Lowercased names of the recording sites. */
  recordingSites: readonly string[];
  /** Whether the status has loaded, so the pill knows the state. */
  statusLoaded: boolean;
  range: InsightsRange;
  paused: boolean;
  busy: InsightsBusy;
  wide: boolean;
}>();

const emit = defineEmits<{
  "update:site": [string];
  "update:range": [InsightsRange];
  toggleRecording: [boolean];
  togglePause: [];
  clear: [];
  settings: [];
}>();

const current = computed(() => props.sites.find((s) => s.name === props.site) ?? null);
const recording = computed(() => !!props.site && props.recordingSites.includes(props.site.toLowerCase()));
const sorted = computed(() => [...props.sites].sort((a, b) => a.name.localeCompare(b.name)));

function sub(s: SiteEntry): string {
  return `${frameworkLabel(s)} · PHP ${s.php}`;
}

function isRecording(s: SiteEntry): boolean {
  return props.recordingSites.includes(s.name.toLowerCase());
}

function onRange(v: string): void {
  if (isRange(v)) emit("update:range", v);
}
</script>

<template>
  <div class="flex items-center gap-2" data-testid="insights-toolbar">
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <button
          type="button"
          :class="
            cn(
              'flex h-8 min-w-0 items-center gap-2 rounded-[7px] border bg-background px-2.5 text-left transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              wide ? 'max-w-[300px]' : 'max-w-[200px]',
            )
          "
          aria-label="Choose a site"
          data-testid="site-picker"
        >
          <Globe class="size-3.5 shrink-0 text-brand" aria-hidden="true" />
          <span class="truncate font-mono text-[13px] font-semibold">
            {{ site ? `${site}.${tld}` : "Choose a site" }}
          </span>
          <span v-if="wide && current" class="shrink-0 truncate text-xs text-faint">{{ sub(current) }}</span>
          <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="max-h-80 w-72 overflow-y-auto">
        <p v-if="sorted.length === 0" class="px-2 py-1.5 text-sm text-muted-foreground">No sites yet</p>
        <DropdownMenuItem
          v-for="s in sorted"
          :key="s.name"
          :data-site="s.name"
          @select="emit('update:site', s.name)"
        >
          <Check :class="cn('size-4 shrink-0', s.name === site ? 'text-brand' : 'opacity-0')" aria-hidden="true" />
          <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{{ s.name }}</span>
          <span
            v-if="isRecording(s)"
            class="size-[7px] shrink-0 rounded-full bg-success"
            title="Recording"
            aria-label="Recording"
          />
          <span class="shrink-0 text-xs text-faint">{{ sub(s) }}</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>

    <button
      v-if="site && statusLoaded"
      type="button"
      :class="
        cn(
          'inline-flex h-7 shrink-0 items-center gap-2 rounded-full px-3 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-60',
          recording ? 'bg-success/15 text-success hover:bg-success/20' : 'border text-muted-foreground hover:bg-accent hover:text-foreground',
        )
      "
      :disabled="busy === 'enabled'"
      :aria-pressed="recording"
      :title="recording ? 'Stop recording this site' : 'Start recording this site'"
      data-testid="recording-pill"
      @click="emit('toggleRecording', !recording)"
    >
      <Spinner v-if="busy === 'enabled'" class="size-3 text-current" />
      <span v-else :class="cn('size-[7px] rounded-full', recording ? 'bg-success' : 'bg-faint')" aria-hidden="true" />
      <template v-if="recording">{{ wide ? "Recording · this site" : "Recording" }}</template>
      <template v-else>{{ wide ? "Not recording" : "Off" }}</template>
    </button>

    <div class="flex-1" />
    <SegmentedControl
      :model-value="range"
      :options="RANGE_OPTIONS"
      variant="plain"
      aria-label="Time range"
      @update:model-value="onRange"
    />
    <Button
      variant="outline"
      size="icon"
      :class="cn('size-8', paused && 'border-brand/45 bg-brand/[0.14] text-brand')"
      :aria-pressed="paused"
      :aria-label="paused ? 'Resume updates' : 'Pause updates'"
      :title="paused ? 'Resume live updates' : 'Pause live updates (the view holds still)'"
      data-testid="pause"
      @click="emit('togglePause')"
    >
      <Play v-if="paused" />
      <Pause v-else />
    </Button>
    <Button
      variant="outline"
      size="icon"
      class="size-8"
      :disabled="!site || busy === 'clear'"
      aria-label="Clear recorded data"
      title="Clear everything recorded for this site"
      data-testid="clear"
      @click="emit('clear')"
    >
      <Spinner v-if="busy === 'clear'" />
      <Trash2 v-else />
    </Button>
    <Button
      variant="outline"
      size="icon"
      class="size-8"
      aria-label="Insights settings"
      title="Thresholds and capture settings"
      data-testid="settings"
      @click="emit('settings')"
    >
      <SlidersHorizontal />
    </Button>
  </div>
</template>
