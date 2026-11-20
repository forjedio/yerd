<script setup lang="ts">
import { Download, Laptop, LoaderCircle, RotateCw, ShieldAlert, Square } from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";

import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import ContributorsCard from "@/components/overview/ContributorsCard.vue";
import FavouritesCard from "@/components/overview/FavouritesCard.vue";
import OverviewStatusCard from "@/components/overview/OverviewStatusCard.vue";
import ReleaseNotesCard from "@/components/overview/ReleaseNotesCard.vue";
import SetupChecklist from "@/components/overview/SetupChecklist.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import {
  daemonVersionConflict,
  IpcError,
  openInBrowser,
  restartDaemon,
  sitesAndParked,
  stopDaemon,
} from "@/ipc/client";
import { useToast } from "@/composables/useToast";
import { emptyEnvironment } from "@/lib/overview";
import logoUrl from "@/assets/logo.svg";

// The home/dashboard. It reads the shared daemon report (no poller of its own)
// and shows the shared daemon-down hero, so it stays useful when the socket is
// gone - the same surface that shows "N sites" becomes the start button. The
// right column (release notes, contributors) loads independently of the left.
const { report, connected, refresh } = useDaemon();
const toast = useToast();

const running = computed(() => connected.value === true);

/** Where the version-conflict banner sends the user: only a newer app build fixes it. */
const RELEASES_URL = "https://github.com/forjedio/yerd/releases";

// ── daemon lifecycle (Stop / Restart; Start lives in the daemon-down hero) ──
const busy = ref<string | null>(null);

const stopDaemonOpen = ref(false);
async function confirmStopDaemon(): Promise<void> {
  busy.value = "daemon";
  stopDaemonOpen.value = false;
  try {
    await stopDaemon();
    toast.success("Stopping daemon…");
  } catch (e) {
    toast.error("Couldn't stop the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refresh();
  }
}

const restartDaemonOpen = ref(false);
/**
 * Restart is fire-and-forget: the daemon acknowledges before it re-execs, so a
 * resolved call means the restart was accepted (the status poll handles the
 * connection drop), while a rejection is a genuine failure worth surfacing.
 */
async function confirmRestartDaemon(): Promise<void> {
  busy.value = "restart:daemon";
  restartDaemonOpen.value = false;
  try {
    await restartDaemon();
    toast.info("Restarting daemon…", "It returns in a few seconds.");
  } catch (e) {
    toast.error("Couldn't restart the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// First paint, before the poll has resolved either way; also the brief window
// where the socket answered but the first report hasn't landed.
const connecting = computed(
  () => (connected.value === null && !report.value) || (running.value && !report.value),
);
const tld = computed(() => report.value?.tld ?? "test");

// ── live site list (favourites and the site count) ──
// Shared "sites" cache (same key + fetcher as the Sites view and the command
// palette). `immediate:false` so it never fetches while the daemon is down (the
// hero shows then); the `running`-gated triggers below drive it instead. A
// failed fetch just leaves the last-good list.
const { data: sitesData, refresh: reloadSites } = useResource("sites", sitesAndParked, {
  immediate: false,
});
const sites = computed(() => sitesData.value?.sites ?? []);
const sitesLoaded = computed(() => sitesData.value !== null);

// macOS: set when this (older) GUI refused to reconfigure a NEWER registered
// daemon. A GUI-vs-registered-daemon condition (independent of reachability), so
// it shows above every branch below.
const versionConflict = ref<string | null>(null);

onMounted(() => {
  if (running.value) void reloadSites();
  daemonVersionConflict()
    .then((v) => (versionConflict.value = v))
    .catch(() => {});
});
// Refetch on (re)connect. On daemon-down do nothing: the display is derived from
// the shared cache and the hero renders regardless, so clearing it would only
// hurt the Sites view / palette that share the entry.
watch(running, (up) => {
  if (up) void reloadSites();
});
usePoll(() => (running.value ? reloadSites() : Promise.resolve()), 5000);

onUnmounted(
  registerViewActions({
    refresh: () => {
      if (running.value) void reloadSites();
    },
  }),
);

// Once the real list is in, drive the count from it so the headline and the
// Sites badge never disagree; fall back to the report's counts until then.
const siteCount = computed(() =>
  sitesLoaded.value
    ? sites.value.length
    : report.value
      ? report.value.sites.parked + report.value.sites.linked
      : 0,
);

const favouritesSkeleton = computed(
  () => connecting.value || (running.value && !!report.value && !sitesLoaded.value),
);

const stopBody = computed(() =>
  siteCount.value > 0
    ? `This stops all ${siteCount.value} .${tld.value} sites, DNS and your databases until you start Yerd again.`
    : `This stops all .${tld.value} sites, DNS and your databases until you start Yerd again.`,
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Overview" docs="/guide/desktop-app" />

    <div
      v-if="versionConflict"
      data-testid="version-conflict"
      class="flex shrink-0 items-center gap-3 border-b border-destructive/30 bg-destructive/10 px-8 py-2.5"
    >
      <ShieldAlert class="size-4 shrink-0 text-destructive" aria-hidden="true" />
      <div class="min-w-0 flex-1">
        <p class="text-[13px] font-semibold">This Yerd is older than your daemon</p>
        <p class="text-xs text-muted-foreground">
          The daemon is registered at
          <span class="font-mono font-semibold text-foreground">{{ versionConflict }}</span>, newer
          than this app. Yerd won't reconfigure or downgrade it. Update this app to
          {{ versionConflict }} or newer.
        </p>
      </div>
      <Button size="sm" class="shrink-0" @click="openInBrowser(RELEASES_URL)">
        <Download /> Update
      </Button>
    </div>

    <div class="flex-1 overflow-y-auto px-6 py-6 xl:px-8">
      <div class="grid gap-6 xl:grid-cols-2 xl:items-start">
        <div class="flex min-w-0 flex-col gap-3.5" data-testid="overview-left">
          <div class="flex h-6 items-center gap-2">
            <Laptop class="size-[15px] text-muted-foreground" aria-hidden="true" />
            <h2 class="text-[15px] font-semibold">Your environment</h2>
          </div>

          <section
            v-if="connecting"
            data-testid="overview-skeleton"
            class="rounded-xl border bg-card px-5 pb-5 pt-[18px]"
          >
            <div class="flex items-center gap-2">
              <LoaderCircle class="size-3.5 animate-spin text-brand" aria-hidden="true" />
              <span class="text-[11px] font-semibold uppercase tracking-[0.6px] text-brand">
                Connecting
              </span>
            </div>
            <div class="mt-4 h-6 w-56 animate-pulse rounded-md bg-muted" />
            <div class="mt-4 h-2.5 w-80 max-w-full animate-pulse rounded bg-muted" />
            <div class="mt-4 h-11 animate-pulse rounded-lg bg-muted" />
          </section>

          <DaemonDownHero v-else-if="!running || !report" class="min-h-[560px]">
            Favourites and services come back once Yerd is running.
          </DaemonDownHero>

          <template v-else>
            <OverviewStatusCard
              :report="report"
              :site-count="siteCount"
              :busy="busy"
              @stop="stopDaemonOpen = true"
              @restart="restartDaemonOpen = true"
            />
            <SetupChecklist
              v-if="emptyEnvironment(report)"
              :report="report"
              :parked-roots="sitesData?.parked.length ?? 0"
            />
            <FavouritesCard
              v-if="sitesLoaded"
              :sites="sites"
              :report="report"
              :total-sites="siteCount"
              @changed="reloadSites"
            />
          </template>

          <section
            v-if="favouritesSkeleton"
            data-testid="favourites-skeleton"
            class="rounded-xl border bg-card px-5 pb-6 pt-[18px]"
          >
            <div class="h-3.5 w-28 animate-pulse rounded bg-muted" />
            <div v-for="i in 4" :key="i" class="mt-6 flex items-center gap-3">
              <div class="size-3.5 animate-pulse rounded-sm bg-muted" />
              <div class="flex-1 space-y-1.5">
                <div class="h-2.5 w-44 animate-pulse rounded bg-muted" />
                <div class="h-2 w-28 animate-pulse rounded bg-muted" />
              </div>
            </div>
          </section>
        </div>

        <div class="flex min-w-0 flex-col gap-3.5" data-testid="overview-right">
          <div class="flex h-6 items-center gap-2">
            <img :src="logoUrl" alt="" class="size-4 rounded-[4px]" />
            <h2 class="font-display text-base font-normal leading-none tracking-[0.4px]">YERD</h2>
          </div>
          <ReleaseNotesCard
            :running="connected"
            :installed-version="report?.daemon_version || undefined"
          />
          <ContributorsCard :running="connected" />
        </div>
      </div>
    </div>

    <ConfirmDialog
      v-model:open="stopDaemonOpen"
      title="Stop the daemon?"
      confirm-label="Stop daemon"
      tone="danger"
      :icon="Square"
      @confirm="confirmStopDaemon"
    >
      {{ stopBody }}
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="restartDaemonOpen"
      title="Restart the daemon?"
      confirm-label="Restart"
      tone="primary"
      :icon="RotateCw"
      @confirm="confirmRestartDaemon"
    >
      Sites, DNS and this connection pause briefly while it restarts. It's back in a few seconds.
    </ConfirmDialog>
  </div>
</template>
