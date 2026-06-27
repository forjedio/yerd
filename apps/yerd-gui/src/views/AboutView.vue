<script setup lang="ts">
import { getVersion } from "@tauri-apps/api/app";
import { Download, FileText, RefreshCw, Stethoscope } from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref } from "vue";

import logoUrl from "@/assets/logo.svg";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  applyUpdate,
  cachedUpdateStatus,
  checkUpdates,
  getDiagnostics,
  getGuiLogs,
  IpcError,
  openInBrowser,
  protocolVersion,
  setUpdateChannel,
  status,
} from "@/ipc/client";
import type { GuiLogs, UpdateChannel, UpdateStatusResponse } from "@/ipc/types";
import { humaniseAgo } from "@/lib/utils";

const toast = useToast();
const { connected } = useDaemon();
const running = computed(() => connected.value === true);

const appVersion = ref("");
const protocol = ref<number | null>(null);
const daemonVersion = ref("");

// ── self-update ──
const channelOptions = [
  { value: "stable", label: "Stable" },
  { value: "edge", label: "Edge (pre-release)" },
] as const;
const updateChannel = ref<UpdateChannel>("stable");
const updateStatus = ref<UpdateStatusResponse | null>(null);
const checkingUpdates = ref(false);
const applyingUpdate = ref(false);

const updateSummary = computed(() => {
  const s = updateStatus.value;
  if (!s) return "";
  if (s.available && s.target) return `Update available: ${s.target}`;
  if (s.ahead_of_stable) return "Up to date (on a pre-release ahead of stable)";
  return "Up to date";
});

// "Last checked …" from the persisted timestamp on the (cached or live) result.
const lastCheckedAgo = computed(() => {
  const at = updateStatus.value?.checked_at_epoch;
  return at ? humaniseAgo(at) : null;
});

// Set once the user triggers a live check or changes the channel, so the slower
// cached prefetch can't clobber fresher state when it resolves later.
let liveUpdateFlow = false;

// Monotonic guard for overlapping live checks. The channel Select stays enabled
// while a check is in flight (only `!running` disables it), so a second channel
// change / check can start before the first resolves. Each check captures the
// current seq before awaiting and only commits if it's still the latest, so an
// older response that resolves last can't overwrite newer state out of order.
let checkSeq = 0;

// Pre-fill the section from the daemon's persisted last-check result (no network)
// so the channel + status + "last checked" show immediately on load.
async function loadCachedUpdate(): Promise<void> {
  try {
    const s = await cachedUpdateStatus();
    if (liveUpdateFlow) return; // a newer check / channel change already won the race
    updateStatus.value = s;
    updateChannel.value = s.channel;
  } catch {
    // Daemon down / older daemon: leave the section in its "Not checked" state.
  }
}

// Notify only on an explicit "Check now" (notify=true). A check triggered any
// other way (channel change) updates the shown status silently.
async function runUpdateCheck(notify = false): Promise<void> {
  liveUpdateFlow = true;
  checkingUpdates.value = true;
  const seq = ++checkSeq;
  try {
    const s = await checkUpdates();
    if (seq !== checkSeq) return; // a newer check superseded this one
    updateStatus.value = s;
    updateChannel.value = s.channel;
    if (!notify) return;
    if (s.available && s.target) {
      toast.success("Update available", `Version ${s.target} can be installed.`);
    } else if (s.ahead_of_stable) {
      toast.info("Up to date", "You're on a pre-release ahead of stable.");
    } else if (s.source === "cached") {
      // The daemon couldn't reach the update server and served its last cached
      // result - don't claim "latest version" off possibly-stale data.
      toast.info(
        "Couldn't reach the update server",
        "Showing the last cached result - you may be offline.",
      );
    } else {
      toast.success("Up to date", "You're on the latest version.");
    }
  } catch (e) {
    if (seq !== checkSeq) return; // superseded; stay quiet, the latest owns the UI
    if (notify) toast.error("Couldn't check for updates", (e as IpcError).message);
  } finally {
    // Only the latest in-flight check clears the spinner; an older one resolving
    // first must leave it spinning until the newer check finishes.
    if (seq === checkSeq) checkingUpdates.value = false;
  }
}

// Download + verify + apply. The app quits during the swap and the applier
// relaunches it, so `applyUpdate` usually never returns; a thrown error means
// staging failed (offline, verification, or a non-writable /Applications).
async function onUpdateNow(): Promise<void> {
  applyingUpdate.value = true;
  try {
    await applyUpdate(updateChannel.value);
  } catch (e) {
    applyingUpdate.value = false;
    toast.error("Couldn't apply the update", (e as IpcError).message);
  }
}

// Persist the channel on change, then re-check so the shown status reflects it.
async function onUpdateChannelChange(value: UpdateChannel): Promise<void> {
  liveUpdateFlow = true;
  const previous = updateChannel.value;
  updateChannel.value = value; // optimistic
  try {
    await setUpdateChannel(value);
    await runUpdateCheck();
  } catch (e) {
    updateChannel.value = previous; // revert on failure
    toast.error("Couldn't change the update channel", (e as IpcError).message);
  }
}

onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch {
    /* not in a Tauri context (e.g. tests) - leave blank */
  }
  try {
    const [p, report] = await Promise.all([protocolVersion(), status()]);
    protocol.value = p;
    daemonVersion.value = report.daemon_version;
  } catch (e) {
    // The page degrades gracefully (daemon fields read "unknown"/"-"), so a
    // down daemon needs no alarm here - only surface a real, unexpected error.
    const err = e as IpcError;
    if (!err.unreachable) toast.error("Couldn't load daemon info", err.message);
  }
  void loadCachedUpdate();
});

// ── GUI Logs dialog ─────────────────────────────────────────────────────────

const logsOpen = ref(false);
const logs = ref<GuiLogs | null>(null);
const activeTab = ref<"gui" | "daemon">("gui");
let logsTimer: ReturnType<typeof setInterval> | undefined;

const activeLines = computed(() =>
  activeTab.value === "gui" ? (logs.value?.guiLog ?? []) : (logs.value?.daemonLog ?? []),
);
const activePath = computed(() =>
  activeTab.value === "gui" ? logs.value?.guiPath : logs.value?.daemonPath,
);

async function fetchLogs(silent = false): Promise<void> {
  try {
    logs.value = await getGuiLogs();
  } catch (e) {
    // The 2s poll passes silent=true so a transient read failure doesn't spam a
    // toast every tick; only the initial/manual load surfaces the error.
    if (!silent) toast.error("Couldn't read logs", (e as IpcError).message);
  }
}

function openLogs(): void {
  logsOpen.value = true;
  void fetchLogs();
  // Light polling while open so a live daemon-start trail streams in.
  stopLogPolling();
  logsTimer = setInterval(() => void fetchLogs(true), 2000);
}

function stopLogPolling(): void {
  if (logsTimer) {
    clearInterval(logsTimer);
    logsTimer = undefined;
  }
}

async function copyActive(): Promise<void> {
  try {
    await navigator.clipboard.writeText(activeLines.value.join("\n"));
    toast.success("Copied logs", "Paste this when reporting the problem.");
  } catch {
    toast.error("Couldn't copy", "Your browser blocked clipboard access.");
  }
}

onUnmounted(stopLogPolling);

// ── Diagnostics dialog ──────────────────────────────────────────────────────

const diagOpen = ref(false);
const diagText = ref("");
const diagLoading = ref(false);

async function openDiagnostics(): Promise<void> {
  diagOpen.value = true;
  diagLoading.value = true;
  diagText.value = "";
  try {
    diagText.value = await getDiagnostics();
  } catch (e) {
    toast.error("Couldn't gather diagnostics", (e as IpcError).message);
    diagText.value = "";
  } finally {
    diagLoading.value = false;
  }
}

async function copyDiagnostics(): Promise<void> {
  try {
    await navigator.clipboard.writeText(diagText.value);
    toast.success("Copied diagnostics", "Paste this when reporting the problem.");
  } catch {
    toast.error("Couldn't copy", "Your browser blocked clipboard access.");
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="About" subtitle="Build info and links" />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <!-- Identity + links -->
      <Card>
        <CardContent class="flex flex-col items-center gap-3 py-8 text-center">
          <img :src="logoUrl" alt="Yerd" class="size-20" />
          <div class="space-y-1">
            <p class="text-lg font-semibold">Yerd</p>
            <p class="text-xs text-muted-foreground">
              A cross-platform local PHP development environment.
            </p>
          </div>
          <div class="flex flex-wrap items-center justify-center gap-x-4 gap-y-1 text-sm">
            <button class="text-brand hover:underline" @click="openInBrowser('https://yerd.app')">
              yerd.app
            </button>
            <button
              class="text-brand hover:underline"
              @click="openInBrowser('https://github.com/forjedio/yerd')"
            >
              GitHub
            </button>
            <button class="text-brand hover:underline" @click="openInBrowser('https://forjed.io')">
              forjed.io
            </button>
          </div>
          <p class="text-xs text-muted-foreground">Licensed under MIT.</p>
        </CardContent>
      </Card>

      <!-- Versions -->
      <Card>
        <CardHeader><CardTitle>Versions</CardTitle></CardHeader>
        <CardContent class="space-y-2 text-sm">
          <div class="flex justify-between">
            <span class="text-muted-foreground">App version</span>
            <span class="font-mono">{{ appVersion || "-" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Daemon version</span>
            <span class="font-mono">{{ daemonVersion || "unknown" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">IPC protocol</span>
            <span class="font-mono">{{ protocol ?? "-" }}</span>
          </div>
        </CardContent>
      </Card>

      <!-- Updates -->
      <Card>
        <CardHeader>
          <CardTitle>Updates</CardTitle>
          <CardDescription>
            Keep Yerd up to date. Choose a release channel and check for new
            versions.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Release channel</p>
              <p class="text-xs text-muted-foreground">
                Stable tracks final releases; Edge opts in to pre-releases and
                release candidates.
              </p>
            </div>
            <Select
              :model-value="updateChannel"
              :options="channelOptions"
              aria-label="Update channel"
              :disabled="!running"
              @update:model-value="(v: UpdateChannel) => onUpdateChannelChange(v)"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div class="min-w-0 text-sm">
              <template
                v-if="
                  updateStatus &&
                  (updateStatus.checked_at_epoch || updateStatus.latest_stable || updateStatus.latest_edge)
                "
              >
                <p class="font-medium">{{ updateSummary }}</p>
                <p class="text-xs text-muted-foreground">
                  Current {{ updateStatus.current }} · stable
                  {{ updateStatus.latest_stable ?? "-" }} · edge
                  {{ updateStatus.latest_edge ?? "-" }}
                </p>
                <p v-if="lastCheckedAgo" class="text-xs text-muted-foreground">
                  Last checked {{ lastCheckedAgo }}.
                </p>
              </template>
              <p v-else class="text-xs text-muted-foreground">
                {{ running ? "Not checked yet." : "Start the daemon to check for updates." }}
              </p>
            </div>
            <Button variant="outline" :disabled="!running || checkingUpdates" @click="runUpdateCheck(true)">
              <Spinner v-if="checkingUpdates" class="size-4" />
              <RefreshCw v-else class="size-4" />
              Check now
            </Button>
          </div>

          <div
            v-if="updateStatus?.available"
            class="flex items-center justify-between gap-4 rounded-lg border border-success/40 bg-success/10 p-3"
          >
            <p class="text-sm">
              <template v-if="applyingUpdate">
                Updating to <strong>{{ updateStatus.target }}</strong> - Yerd will restart…
              </template>
              <template v-else>
                Yerd <strong>{{ updateStatus.target }}</strong> is available.
              </template>
            </p>
            <Button :disabled="!running || applyingUpdate" @click="onUpdateNow">
              <Spinner v-if="applyingUpdate" class="size-4" />
              <Download v-else class="size-4" />
              {{ applyingUpdate ? "Updating…" : "Update" }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Troubleshooting -->
      <Card>
        <CardHeader><CardTitle>Troubleshooting</CardTitle></CardHeader>
        <CardContent class="space-y-3">
          <p class="text-sm text-muted-foreground">
            The GUI logs everything it does this session - including the daemon
            install, upgrade, and start steps. Open the logs to see what
            happened, or generate a diagnostics snapshot to share when reporting
            a problem.
          </p>
          <div class="flex flex-wrap justify-end gap-2">
            <Button variant="outline" @click="openLogs">
              <FileText class="size-4" /> Logs
            </Button>
            <Button variant="outline" @click="openDiagnostics">
              <Stethoscope class="size-4" /> Diagnostics
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>

    <Modal
      v-model:open="logsOpen"
      title="Logs"
      size="full"
      @update:open="(o: boolean) => { if (!o) stopLogPolling(); }"
    >
      <div class="flex h-full flex-col gap-3">
        <!-- Tabs + actions -->
        <div class="flex shrink-0 items-center justify-between gap-2">
          <div class="flex gap-1">
            <Button
              :variant="activeTab === 'gui' ? 'default' : 'outline'"
              size="sm"
              @click="activeTab = 'gui'"
            >
              GUI
            </Button>
            <Button
              :variant="activeTab === 'daemon' ? 'default' : 'outline'"
              size="sm"
              @click="activeTab = 'daemon'"
            >
              Daemon
            </Button>
          </div>
          <div class="flex gap-1">
            <Button variant="outline" size="sm" @click="fetchLogs()">Refresh</Button>
            <Button variant="outline" size="sm" @click="copyActive">Copy</Button>
          </div>
        </div>
        <p v-if="activePath" class="shrink-0 truncate font-mono text-xs text-muted-foreground">
          {{ activePath }}
        </p>
        <pre
          class="min-h-0 flex-1 overflow-auto rounded-md bg-muted p-3 text-xs leading-relaxed"
        >{{ activeLines.length ? activeLines.join("\n") : "No log entries yet." }}</pre>
      </div>

      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Close</Button>
      </template>
    </Modal>

    <Modal v-model:open="diagOpen" title="Diagnostics" size="full">
      <div class="flex h-full flex-col gap-3">
        <div class="flex shrink-0 items-center justify-between gap-2">
          <p class="text-xs text-muted-foreground">
            A snapshot of paths, service configuration, and recent errors.
          </p>
          <Button variant="outline" size="sm" :disabled="diagLoading" @click="copyDiagnostics">
            Copy
          </Button>
        </div>
        <pre
          class="min-h-0 flex-1 overflow-auto rounded-md bg-muted p-3 text-xs leading-relaxed"
        >{{ diagLoading ? "Gathering…" : diagText || "No diagnostics available." }}</pre>
      </div>

      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Close</Button>
      </template>
    </Modal>
  </div>
</template>
