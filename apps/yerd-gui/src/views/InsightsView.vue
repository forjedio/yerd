<script setup lang="ts">
import { Activity, Gauge, Play, Trash2 } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import InsightsIssueList from "@/components/insights/InsightsIssueList.vue";
import InsightsKpiTiles from "@/components/insights/InsightsKpiTiles.vue";
import InsightsLatencyChart from "@/components/insights/InsightsLatencyChart.vue";
import InsightsLiveFeed from "@/components/insights/InsightsLiveFeed.vue";
import InsightsRequestDetail from "@/components/insights/InsightsRequestDetail.vue";
import InsightsRequestList from "@/components/insights/InsightsRequestList.vue";
import InsightsRoutesPanel from "@/components/insights/InsightsRoutesPanel.vue";
import InsightsSettingsDialog from "@/components/insights/InsightsSettingsDialog.vue";
import InsightsTimeBreakdown from "@/components/insights/InsightsTimeBreakdown.vue";
import InsightsToolbar from "@/components/insights/InsightsToolbar.vue";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import EmptyState from "@/components/ui/EmptyState.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { type InsightsTab, useInsights } from "@/composables/useInsights";
import { useResource } from "@/composables/useResource";
import { useWideLayout } from "@/composables/useWideLayout";
import { sitesAndParked } from "@/ipc/client";
import type { InsightsIssue, InsightsRequestSummary, InsightsThresholds } from "@/ipc/types";
import { formatCount, type IssueChipKey, issueFilter, timingOnly } from "@/lib/insights";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { CHIP_TONE, cn } from "@/lib/utils";

const router = useRouter();
const wide = useWideLayout();
const { report } = useDaemon();
const tld = computed(() => report.value?.tld ?? "test");

const { data: sitesData } = useResource("sites", sitesAndParked);
const sites = computed(() => sitesData.value?.sites ?? []);

const insights = useInsights();
const {
  busy,
  detail,
  detailError,
  detailLoading,
  overview,
  paused,
  query,
  range,
  requests,
  requestsLoaded,
  routesTab,
  selectedId,
  site,
  slowest,
  sort,
  status,
  tab,
} = insights;

const live = ref<InstanceType<typeof InsightsLiveFeed> | null>(null);

onUnmounted(
  registerViewActions({
    refresh: () => void (tab.value === "live" ? live.value?.refresh() : insights.refresh()),
  }),
);

const recordingSites = computed(() => status.value?.sites ?? []);
const recording = computed(() => insights.recording());
const issueFilterKey = ref<IssueChipKey>("all");
const openIssues = computed(() => (overview.value ? issueFilter(overview.value.issues, "all").length : 0));
const hasData = computed(() => (overview.value?.kpis.requests ?? 0) > 0);
const maxQueries = computed(() => status.value?.thresholds.max_queries ?? 50);

watch(
  [sites, recordingSites],
  ([list, rec]) => {
    if (list.length === 0) return;
    if (site.value && list.some((s) => s.name === site.value)) return;
    const pick = list.find((s) => rec.includes(s.name.toLowerCase())) ?? [...list].sort((a, b) => a.name.localeCompare(b.name))[0];
    if (pick) insights.setSite(pick.name);
  },
  { immediate: true },
);

const TABS = computed<{ key: InsightsTab; label: string; count?: string; pill?: string }[]>(() => [
  { key: "overview", label: "Overview" },
  { key: "requests", label: "Requests", count: overview.value ? formatCount(overview.value.kpis.requests) : undefined },
  {
    key: "issues",
    label: "Issues",
    count: overview.value ? String(openIssues.value) : undefined,
    pill: openIssues.value ? CHIP_TONE.warning : CHIP_TONE.neutral,
  },
  { key: "live", label: "Live Feed" },
]);

const drawerOpen = ref(false);

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

watch(
  [requests, () => tab.value, wide],
  () => {
    if (tab.value !== "requests" || !wide.value || selectedId.value) return;
    const first = requests.value[0];
    if (first) void insights.select(first);
  },
);

function openRequest(r: { id: string; ts_ms: number }): void {
  tab.value = "requests";
  void insights.select(r);
  if (!wide.value) drawerOpen.value = true;
}

function onSelect(r: InsightsRequestSummary): void {
  void insights.select(r);
  if (!wide.value) drawerOpen.value = true;
}

function openIssue(i: InsightsIssue): void {
  if (i.last_request_id && i.last_request_ts_ms != null) openRequest({ id: i.last_request_id, ts_ms: i.last_request_ts_ms });
}

const confirmClear = ref(false);

function togglePause(): void {
  paused.value = !paused.value;
}

const settingsOpen = ref(false);

async function saveThresholds(t: InsightsThresholds): Promise<void> {
  if (await insights.setThresholds(t)) settingsOpen.value = false;
}

function changePort(): void {
  settingsOpen.value = false;
  void router.push("/general");
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Insights" />

    <div class="flex h-12 shrink-0 items-center border-b px-6" data-testid="insights-toolbar-bar">
      <InsightsToolbar
        class="w-full"
        :sites="sites"
        :site="site"
        :tld="tld"
        :recording-sites="recordingSites"
        :status-loaded="!!status"
        :range="range"
        :paused="paused"
        :busy="busy"
        :wide="wide"
        @update:site="insights.setSite"
        @update:range="range = $event"
        @toggle-recording="insights.setEnabled"
        @toggle-pause="togglePause"
        @clear="confirmClear = true"
        @settings="settingsOpen = true"
      />
    </div>

    <nav class="flex h-12 shrink-0 items-end gap-7 border-b px-6" aria-label="Insights sections">
      <button
        v-for="t in TABS"
        :key="t.key"
        type="button"
        :class="
          cn(
            '-mb-px flex h-11 items-center gap-2 border-b-2 text-[15px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            tab === t.key ? 'border-primary font-semibold text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground',
          )
        "
        :aria-current="tab === t.key ? 'page' : undefined"
        :data-tab="t.key"
        @click="tab = t.key"
      >
        {{ t.label }}
        <span
          v-if="t.count && t.pill"
          :class="
            cn(
              'inline-flex h-5 min-w-[22px] items-center justify-center rounded-full px-1.5 font-mono text-[11px] font-semibold',
              t.pill,
            )
          "
          data-testid="issues-tab-count"
        >
          {{ t.count }}
        </span>
        <span v-else-if="t.count" class="font-mono text-xs text-faint">{{ t.count }}</span>
      </button>
    </nav>

    <div v-if="sitesData && sites.length === 0" class="p-6">
      <EmptyState :icon="Gauge" title="No sites yet" description="Link or park a PHP site, then record its requests here." />
    </div>

    <InsightsLiveFeed v-else-if="tab === 'live'" ref="live" :site="site" :wide="wide" />

    <div v-else-if="!status || (!overview && tab !== 'requests')" class="flex flex-1 items-center justify-center">
      <Spinner />
    </div>

    <div v-else-if="!recording && !hasData && tab !== 'requests'" class="p-6" data-testid="start-recording">
      <EmptyState
        :icon="Activity"
        title="Insights is off for this site"
        description="Record every request's timing, queries and problems like N+1s. Other sites are unaffected."
      >
        <Button :disabled="busy === 'enabled'" data-testid="start-recording-button" @click="insights.setEnabled(true)">
          <Spinner v-if="busy === 'enabled'" class="text-current" />
          <Play v-else />
          Start recording
        </Button>
      </EmptyState>
    </div>

    <div v-else-if="tab === 'overview' && overview && !hasData" class="p-6" data-testid="waiting">
      <EmptyState
        :icon="Activity"
        title="Waiting for the first request"
        :description="`Load any page on ${site}.${tld}. Static files are skipped.`"
      />
    </div>

    <div v-else-if="tab === 'overview' && overview" class="min-h-0 flex-1 overflow-y-auto" data-testid="overview-tab">
      <div class="flex flex-col gap-4 px-6 py-5">
        <InfoBanner v-if="timingOnly(overview)" data-testid="timing-only">
          Timing only: install or update the Yerd PHP extension to see queries, dumps and cold starts.
        </InfoBanner>
        <InfoBanner v-if="!recording" variant="warning">
          Recording is off for this site. You are looking at what was recorded before.
        </InfoBanner>
        <InsightsKpiTiles :overview="overview" :max-queries="maxQueries" :compact="!wide" />
        <div :class="cn('grid gap-4', wide ? 'grid-cols-[minmax(0,1.85fr)_minmax(0,1fr)]' : 'grid-cols-1')">
          <InsightsLatencyChart :series="overview.series" :spike-at-ms="overview.kpis.p95_spike_at_ms" />
          <InsightsTimeBreakdown :slices="overview.breakdown" />
        </div>
        <InsightsIssueList
          v-model:filter="issueFilterKey"
          :issues="overview.issues"
          :busy="busy"
          :limit="5"
          @open="openIssue"
          @dismiss="insights.dismiss"
        />
        <InsightsRoutesPanel
          v-model:tab="routesTab"
          :routes="overview.routes"
          :queries="overview.queries"
          :slowest="slowest"
          @open="openRequest"
        />
      </div>
    </div>

    <div v-else-if="tab === 'issues' && overview" class="min-h-0 flex-1 overflow-y-auto" data-testid="issues-tab">
      <div class="px-6 py-5">
        <InsightsIssueList
          v-model:filter="issueFilterKey"
          :issues="overview.issues"
          :busy="busy"
          @open="openIssue"
          @dismiss="insights.dismiss"
        />
      </div>
    </div>

    <div v-else-if="tab === 'requests'" class="flex min-h-0 flex-1" :data-layout="wide ? 'wide' : 'compact'" data-testid="requests-tab">
      <InsightsRequestList
        v-model:query="query"
        v-model:sort="sort"
        :class="wide ? 'w-[440px] shrink-0 border-r' : 'min-w-0 flex-1'"
        :layout="wide ? 'list' : 'table'"
        :requests="requests"
        :selected="wide ? selectedId : null"
        :loaded="requestsLoaded"
        @select="onSelect"
      />
      <div :class="wide ? 'min-w-0 flex-1' : 'hidden'">
        <InspectorFrame
          :mode="wide ? 'panel' : 'drawer'"
          :shown="wide ? true : drawerOpen && !!selectedId"
          label="Request"
          testid="insights-request-detail"
          wide-drawer
          @close="drawerOpen = false"
        >
          <InsightsRequestDetail
            :detail="detail"
            :loading="detailLoading"
            :error="detailError"
            :closable="!wide"
            @close="drawerOpen = false"
          />
        </InspectorFrame>
      </div>
    </div>

    <InsightsSettingsDialog
      v-model:open="settingsOpen"
      :thresholds="status?.thresholds ?? null"
      :saving="busy === 'thresholds'"
      @save="saveThresholds"
      @change-port="changePort"
    />

    <ConfirmDialog
      v-model:open="confirmClear"
      title="Clear Insights for this site?"
      confirm-label="Clear"
      tone="danger"
      :icon="Trash2"
      @confirm="insights.clear"
    >
      Every recorded request, chart point and dismissed issue for {{ site }}.{{ tld }} is deleted. Recording stays
      {{ recording ? "on" : "off" }}.
    </ConfirmDialog>
  </div>
</template>
