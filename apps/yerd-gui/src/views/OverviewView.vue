<script setup lang="ts">
import {
  ArrowUpRight,
  CircleAlert,
  Database,
  Download,
  Github,
  Link,
  LoaderCircle,
  Plus,
  RotateCw,
  ShieldAlert,
  Square,
} from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import ContributorsCard from "@/components/overview/ContributorsCard.vue";
import FavouritesCard from "@/components/overview/FavouritesCard.vue";
import OverviewStatusCard from "@/components/overview/OverviewStatusCard.vue";
import ReleaseNotesCard from "@/components/overview/ReleaseNotesCard.vue";
import ServicesCard from "@/components/overview/ServicesCard.vue";
import SetupChecklist from "@/components/overview/SetupChecklist.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { servicesIntent } from "@/lib/shortcuts/servicesIntent";
import { sitesIntent, type SitesIntent } from "@/lib/shortcuts/sitesIntent";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useUpdateCheck } from "@/composables/useUpdateCheck";
import {
  daemonVersionConflict,
  IpcError,
  openInBrowser,
  restartDaemon,
  sitesAndParked,
  stopDaemon,
  tunnelStatus,
} from "@/ipc/client";
import { useToast } from "@/composables/useToast";
import { setupPending, setupSteps } from "@/lib/overview";
import { sharedSiteNames } from "@/lib/share";

// The home/dashboard. It reads the shared daemon report (no poller of its own)
// and shows the shared daemon-down hero, so it stays useful when the socket is
// gone - the same surface that shows "N sites" becomes the start button.
// Contributors and What's new load independently of the rest.
const { report, connected, lastError, refresh } = useDaemon();
const toast = useToast();
const router = useRouter();
const { home } = usePlatform();
const updates = useUpdateCheck();

const running = computed(() => connected.value === true);

/** Where the version-conflict banner sends the user: only a newer app build fixes it. */
const RELEASES_URL = "https://github.com/forjedio/yerd/releases";
const REPO_URL = "https://github.com/forjedio/yerd";

function openSites(intent: SitesIntent): void {
  sitesIntent.value = intent;
  void router.push("/sites");
}

function addService(): void {
  servicesIntent.value = "add";
  void router.push("/services");
}

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

/** The daemon answered but its status couldn't be read (a typed error, e.g. a
 *  GUI/daemon version skew the report doesn't decode across), so there is no
 *  report to show and none is on its way. */
const statusError = computed(() =>
  running.value && !report.value ? (lastError.value?.message ?? null) : null,
);

// First paint, before the poll has resolved either way; also the brief window
// where the socket answered but the first report hasn't landed.
const connecting = computed(
  () =>
    (connected.value === null && !report.value) ||
    (running.value && !report.value && statusError.value === null),
);

const retrying = ref(false);
async function retryStatus(): Promise<void> {
  retrying.value = true;
  try {
    await refresh();
  } finally {
    retrying.value = false;
  }
}
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
const groups = computed(() => sitesData.value?.groups ?? { order: [], members: {} });

// Tunnel status is only fetched while the report counts a share, so the common
// case costs no extra IPC. The cached list is dropped when sharing stops, so a
// stale one never flashes a badge when the next share starts.
const {
  data: tunnelsData,
  refresh: reloadTunnels,
  mutate: setTunnels,
} = useResource("overview-tunnels", tunnelStatus, {
  immediate: false,
});
const sharing = computed(() => (report.value?.shared_sites ?? 0) > 0);
const shared = computed(() =>
  sharing.value && tunnelsData.value ? sharedSiteNames(tunnelsData.value.tunnels) : new Set<string>(),
);
function reloadSharing(): Promise<void> {
  return running.value && sharing.value ? reloadTunnels() : Promise.resolve();
}
watch(
  sharing,
  (on) => {
    if (!on) setTunnels(() => null);
    void reloadSharing();
  },
  { immediate: true },
);

// macOS: set when this (older) GUI refused to reconfigure a NEWER registered
// daemon. A GUI-vs-registered-daemon condition (independent of reachability), so
// it shows above every branch below.
const versionConflict = ref<string | null>(null);

onMounted(() => {
  void loadPlatform();
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
usePoll(
  () => (running.value ? Promise.all([reloadSites(), reloadSharing()]).then(() => {}) : Promise.resolve()),
  5000,
);

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

const showSetup = computed(
  () => !!report.value && setupPending(setupSteps(report.value, sitesData.value?.parked.length ?? 0)),
);

/** The Pinned/Services grid replaces the hero only while the daemon is up (or still connecting). */
const showGrid = computed(
  () => connecting.value || (running.value && !!report.value && statusError.value === null),
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

    <div class="flex-1 overflow-y-auto px-6 py-6 xl:px-7">
      <div class="flex min-w-0 flex-col gap-4">
        <div data-testid="overview-actions" class="flex items-center gap-2">
          <Button size="sm" class="h-[34px] px-3.5" :disabled="!running" @click="openSites('create')">
            <Plus /> New site
          </Button>
          <Button
            size="sm"
            variant="outline"
            class="h-[34px] px-3"
            :disabled="!running"
            @click="openSites('link')"
          >
            <Link class="!size-3.5" /> Link folder
          </Button>
          <Button size="sm" variant="outline" class="h-[34px] px-3" :disabled="!running" @click="addService">
            <Database class="!size-3.5" /> Add service
          </Button>
          <div class="ml-auto flex items-center gap-1">
            <Button
              size="sm"
              variant="ghost"
              class="h-[30px] px-2 text-muted-foreground"
              aria-label="Check for updates"
              title="Check for updates"
              :disabled="!running || updates.checking.value"
              @click="updates.check(true)"
            >
              <Spinner v-if="updates.checking.value" class="!size-3.5" />
              <Download v-else class="!size-3.5" />
              <span class="hidden xl:inline">Check for updates</span>
            </Button>
            <Button
              size="sm"
              variant="ghost"
              class="h-[30px] px-2 text-muted-foreground"
              aria-label="GitHub"
              title="Open Yerd on GitHub"
              @click="openInBrowser(REPO_URL)"
            >
              <Github class="!size-3.5" />
              <span class="hidden xl:inline">GitHub</span>
              <ArrowUpRight class="hidden !size-3 xl:inline" />
            </Button>
          </div>
        </div>

        <section
          v-if="connecting"
          data-testid="overview-skeleton"
          class="rounded-xl border bg-muted px-5 py-4 dark:bg-card"
        >
          <div class="flex items-center gap-2">
            <LoaderCircle class="size-3.5 animate-spin text-brand" aria-hidden="true" />
            <span class="text-[11px] font-semibold uppercase tracking-[0.6px] text-brand">
              Connecting
            </span>
          </div>
          <div class="mt-3 h-6 w-56 animate-pulse rounded-md bg-card dark:bg-background" />
          <div class="mt-3 h-2.5 w-80 max-w-full animate-pulse rounded bg-card dark:bg-background" />
        </section>

        <section
          v-else-if="statusError !== null"
          data-testid="overview-status-error"
          role="alert"
          class="rounded-xl border border-destructive/40 bg-card px-5 pb-5 pt-[18px]"
        >
          <div class="flex items-center gap-2">
            <CircleAlert class="size-4 text-destructive" aria-hidden="true" />
            <h3 class="text-[15px] font-semibold">Couldn't read the daemon's status</h3>
          </div>
          <p class="mt-2 break-words text-[13px] text-muted-foreground">{{ statusError }}</p>
          <p class="mt-1 text-xs text-faint">
            The daemon is running but its reply didn't make sense to this app. If you've just
            updated, restarting Yerd usually fixes it.
          </p>
          <div class="mt-4 flex gap-2">
            <Button size="sm" :disabled="retrying" @click="retryStatus">
              <RotateCw /> Retry
            </Button>
            <Button
              size="sm"
              variant="outline"
              :disabled="busy !== null"
              @click="restartDaemonOpen = true"
            >
              Restart daemon
            </Button>
          </div>
        </section>

        <DaemonDownHero v-else-if="!running || !report" class="min-h-[480px]">
          Your sites and services come back once Yerd is running.
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
            v-if="showSetup"
            :report="report"
            :parked-roots="sitesData?.parked.length ?? 0"
          />
        </template>

        <div
          v-if="showGrid"
          data-testid="overview-grid"
          class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_436px] xl:items-stretch"
        >
          <FavouritesCard
            v-if="report && !favouritesSkeleton"
            :sites="sites"
            :report="report"
            :total-sites="siteCount"
            :groups="groups"
            :home="home"
            :shared="shared"
            @changed="reloadSites({ force: true })"
          />
          <section
            v-else
            data-testid="favourites-skeleton"
            class="rounded-xl border bg-muted px-5 pb-6 pt-[18px] dark:bg-card"
          >
            <div class="h-3.5 w-28 animate-pulse rounded bg-card dark:bg-background" />
            <div v-for="i in 4" :key="i" class="mt-6 flex items-center gap-3">
              <div class="size-3.5 animate-pulse rounded-sm bg-card dark:bg-background" />
              <div class="flex-1 space-y-1.5">
                <div class="h-2.5 w-44 animate-pulse rounded bg-card dark:bg-background" />
                <div class="h-2 w-28 animate-pulse rounded bg-card dark:bg-background" />
              </div>
            </div>
          </section>

          <div class="flex min-w-0 flex-col gap-4">
            <ServicesCard v-if="report" :services="report.services ?? []" :tld="tld" :sites="sites" />
            <ContributorsCard :running="connected" />
          </div>
        </div>
        <ContributorsCard v-else :running="connected" />

        <ReleaseNotesCard
          :running="connected"
          :installed-version="report?.daemon_version || undefined"
        />
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
