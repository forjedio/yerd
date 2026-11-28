<script setup lang="ts">
import { ChevronRight, RotateCcw, X } from "lucide-vue-next";
import { computed } from "vue";

import FilterChips from "@/components/ui/FilterChips.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { CARD } from "@/components/ui/styles";
import type { InsightsBusy } from "@/composables/useInsights";
import type { InsightsIssue } from "@/ipc/types";
import { TAG } from "@/lib/dumps";
import {
  ISSUE_LABEL,
  ISSUE_TONE,
  issueChips,
  issueDetail,
  issueFilter,
  issueImpact,
  issueTitle,
  methodTone,
  type IssueChipKey,
} from "@/lib/insights";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * The detected issues: a card with a count, type chips (All, each kind with
 * open issues, Dismissed) filtered client-side, and one row per issue with its
 * SQL, what happened and where, the route and the time it costs. A row opens
 * the latest affected request; the hover action dismisses or restores it.
 */
const props = defineProps<{
  issues: readonly InsightsIssue[];
  busy: InsightsBusy;
  /** Cap the rows (the Overview card), or show them all (the Issues tab). */
  limit?: number;
}>();

const filter = defineModel<IssueChipKey>("filter", { required: true });

const emit = defineEmits<{
  open: [issue: InsightsIssue];
  dismiss: [key: string, dismissed: boolean];
}>();

const open = computed(() => issueFilter(props.issues, "all").length);
const chips = computed(() => issueChips(props.issues, filter.value));
const shown = computed(() => {
  const rows = issueFilter(props.issues, filter.value);
  return props.limit ? rows.slice(0, props.limit) : rows;
});
const hidden = computed(() => issueFilter(props.issues, filter.value).length - shown.value.length);

const COLS = "grid grid-cols-[112px_minmax(0,1fr)_minmax(0,200px)_72px_56px] items-center gap-x-4";
</script>

<template>
  <section :class="cn(CARD, 'flex min-w-0 flex-col')" data-testid="issue-list">
    <header class="flex min-h-[52px] items-center gap-3 border-b px-5 py-2">
      <h3 class="font-mono text-[11px] uppercase tracking-[1.4px] text-faint">Detected issues</h3>
      <span
        :class="
          cn(
            'inline-flex h-5 min-w-[22px] items-center justify-center rounded-full px-1.5 font-mono text-[11px] font-semibold',
            open > 0 ? CHIP_TONE.warning : CHIP_TONE.neutral,
          )
        "
        data-testid="issue-count"
      >
        {{ open }}
      </span>
      <FilterChips v-model="filter" class="ml-auto" :chips="chips" label="Filter issues" />
    </header>

    <p v-if="shown.length === 0" class="px-5 py-8 text-center text-[13px] text-muted-foreground" data-testid="issues-empty">
      <template v-if="filter === 'dismissed'">Nothing dismissed.</template>
      <template v-else-if="issues.length === 0">No issues detected in this range.</template>
      <template v-else>No open issues of this type.</template>
    </p>

    <ul v-else class="flex flex-col">
      <li
        v-for="i in shown"
        :key="i.key"
        :class="cn(COLS, 'group relative border-b px-5 py-3 last:border-b-0', i.dismissed && 'opacity-60')"
        :data-issue="i.key"
      >
        <span :class="cn(TAG, CHIP_TONE[ISSUE_TONE[i.kind]], 'h-6 w-full justify-center uppercase tracking-[0.4px]')">
          {{ ISSUE_LABEL[i.kind] }}
        </span>
        <button
          type="button"
          class="min-w-0 rounded-sm text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-default"
          :disabled="!i.last_request_id"
          :title="i.last_request_id ? 'Open the latest affected request' : undefined"
          @click="emit('open', i)"
        >
          <span class="block truncate font-mono text-[13px] font-medium">{{ issueTitle(i) }}</span>
          <span class="mt-0.5 block truncate text-xs text-muted-foreground" :title="issueDetail(i)">{{ issueDetail(i) }}</span>
        </button>
        <span class="flex min-w-0 items-center gap-2">
          <span v-if="i.route_method" :class="cn(TAG, CHIP_TONE[methodTone(i.route_method)])">{{ i.route_method }}</span>
          <span class="truncate font-mono text-[13px]">{{ i.route_label }}</span>
        </span>
        <span
          :class="
            cn(
              'text-right font-mono text-[13px]',
              i.kind === 'slow_query' ? 'text-destructive' : i.kind === 'n_plus_one' ? 'text-warning' : 'text-muted-foreground',
            )
          "
        >
          {{ issueImpact(i) }}
        </span>
        <span class="flex items-center justify-end gap-1">
          <button
            type="button"
            class="flex size-6 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring group-hover:opacity-100"
            :aria-label="i.dismissed ? 'Restore issue' : 'Dismiss issue'"
            :title="i.dismissed ? 'Restore' : 'Dismiss'"
            :disabled="busy === `dismiss:${i.key}`"
            data-testid="dismiss"
            @click="emit('dismiss', i.key, !i.dismissed)"
          >
            <Spinner v-if="busy === `dismiss:${i.key}`" class="size-3.5" />
            <RotateCcw v-else-if="i.dismissed" class="size-3.5" />
            <X v-else class="size-3.5" />
          </button>
          <ChevronRight v-if="i.last_request_id" class="size-4 text-faint" aria-hidden="true" />
        </span>
      </li>
    </ul>
    <p v-if="hidden > 0" class="border-t px-5 py-2.5 text-xs text-muted-foreground">
      {{ hidden }} more in the Issues tab
    </p>
  </section>
</template>
