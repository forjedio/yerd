<script setup lang="ts">
import {
  Download,
  Info,
  Play,
  RefreshCw,
  RotateCw,
  Square,
} from "lucide-vue-next";
import { computed, nextTick, onMounted, ref, watch } from "vue";

import DaemonDiagnosticsPanel from "@/components/DaemonDiagnosticsPanel.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useDaemon } from "@/composables/useDaemon";
import { useDaemonStart } from "@/composables/useDaemonStart";
import { useToast } from "@/composables/useToast";
import {
  applyUpdate,
  checkUpdates,
  cliPathStatus,
  dumpsStatus,
  getAutostart,
  hostPlatform,
  installCliToPath,
  IpcError,
  openLoginItems,
  removeCliFromPath,
  restartDaemon,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
  setUpdateChannel,
  stopDaemon,
} from "@/ipc/client";
import type {
  AutostartState,
  CliPathStatus,
  DumpsStatusResponse,
  StatusReport,
  UpdateChannel,
  UpdateStatusResponse,
} from "@/ipc/types";
import { useTheme, type ThemePref } from "@/lib/theme";
import { humaniseBytes, humaniseUptime } from "@/lib/utils";

const { connected, report, refresh: refreshStatus } = useDaemon();
// Surfaces the same failure diagnostics here as onboarding / the down-hero, so a
// start attempt from Settings (a screen shown while the daemon is down) isn't a
// blind toast.
const {
  starting: daemonStarting,
  diagnostics: startDiagnostics,
  start: startDaemonFlow,
} = useDaemonStart();
const toast = useToast();
const { pref, setTheme } = useTheme();

const busy = ref<string | null>(null);
const autostart = ref<AutostartState | null>(null);
// Host platform — drives macOS-specific daemon copy (on macOS the daemon runs
// as a background login item registered via SMAppService; see below).
const platform = ref("");
const isMac = computed(() => platform.value === "macos");
// macOS-only: whether the bundled `yerd` CLI is symlinked onto PATH.
const cli = ref<CliPathStatus | null>(null);
// Dump-capture status (separate IPC from the report) — feeds the subsystem list.
const dumps = ref<DumpsStatusResponse | null>(null);

const themeOptions = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
] as const;

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

const running = computed(() => connected.value === true);
const daemonStatus = computed(() => {
  if (!running.value) return "Stopped";
  const pid = report.value?.daemon_pid;
  return pid ? `Running · pid ${pid}` : "Running";
});

// ── subsystem table (daemon + the in-process DNS/proxy listeners; no FPM) ──
interface Row {
  key: string;
  name: string;
  tone: Tone;
  state: string;
  info: string;
  child?: boolean; // indented under the daemon (runs inside its process)
  menu?: boolean; // the daemon row gets the Restart action
}

const PRIVILEGED_PORT_CEILING = 1024;

function portRow(
  key: string,
  name: string,
  r: StatusReport,
  which: "http" | "https",
): Row {
  const ps = r[which];
  const privileged = ps.requested < PRIVILEGED_PORT_CEILING;
  const redirected = r.port_redirect === true;
  const viaRedirect = privileged && ps.fell_back && redirected;
  const problem = privileged && ps.fell_back && !redirected;

  let detail: string;
  if (!ps.fell_back) {
    detail = "privileged port";
  } else if (viaRedirect) {
    detail = `:${ps.requested} via pf redirect → :${ps.bound}`;
  } else {
    detail = `rootless fallback from :${ps.requested}`;
  }

  return {
    key,
    name,
    tone: problem ? "warn" : "ok",
    state: viaRedirect ? `:${ps.requested}` : `:${ps.bound}`,
    info: detail,
    child: true,
  };
}

function mailRow(r: StatusReport): Row | null {
  const m = r.mail;
  if (!m) return null; // daemon predates the mail subsystem
  let tone: Tone;
  let state: string;
  let info: string;
  if (!m.enabled) {
    tone = "muted";
    state = "disabled";
    info = "enable it on the Mail page";
  } else if (m.listening) {
    tone = "ok";
    state = `:${m.port}`;
    info = `SMTP on 127.0.0.1:${m.port} · ${m.count} captured`;
  } else {
    tone = "warn";
    state = `:${m.port}`;
    info = `port :${m.port} unavailable`;
  }
  return { key: "mail", name: "Mail capture", tone, state, info, child: true };
}

// The dump-capture loopback server is a daemon subsystem like mail/DNS, but its
// state lives in a separate IPC (dumps_status), fetched alongside the report.
function dumpsRow(d: DumpsStatusResponse | null): Row | null {
  if (!d) return null; // daemon predates the dumps subsystem / fetch failed
  let tone: Tone;
  let state: string;
  let info: string;
  if (!d.enabled) {
    tone = "muted";
    state = "disabled";
    info = "enable capture on the Dumps page";
  } else if (d.running) {
    tone = "ok";
    state = `:${d.port}`;
    info = `dump server on 127.0.0.1:${d.port}`;
  } else {
    tone = "warn";
    state = `:${d.port}`;
    info = `port :${d.port} unavailable`;
  }
  return { key: "dumps", name: "Dump capture", tone, state, info, child: true };
}

const rows = computed<Row[]>(() => {
  const r = report.value;
  if (!r) return [];
  const base: Row[] = [
    {
      key: "daemon",
      name: "Daemon (yerdd)",
      tone: "ok",
      state: "running",
      info: `pid ${r.daemon_pid} · up ${humaniseUptime(r.uptime_secs)}${
        r.daemon_rss_bytes != null ? ` · ${humaniseBytes(r.daemon_rss_bytes)} RSS` : ""
      }`,
      menu: true,
    },
    {
      key: "dns",
      name: "DNS resolver",
      tone: "ok",
      state: "listening",
      info: `bound on ${r.dns_addr}`,
      child: true,
    },
    portRow("proxy-http", "Proxy (HTTP)", r, "http"),
    portRow("proxy-https", "Proxy (HTTPS)", r, "https"),
  ];
  const mail = mailRow(r);
  if (mail) base.push(mail);
  const dump = dumpsRow(dumps.value);
  if (dump) base.push(dump);
  return base;
});

// ── data loads ──
async function loadAutostart(): Promise<void> {
  try {
    autostart.value = await getAutostart();
  } catch (e) {
    toast.error("Couldn't load startup settings", (e as IpcError).message);
  }
}

/** Dump-capture status is a separate IPC; a failure just hides the row. */
async function loadDumps(): Promise<void> {
  try {
    dumps.value = await dumpsStatus();
  } catch {
    dumps.value = null;
  }
}

// ── CLI on PATH (macOS) + Login-Items approval ──
async function loadCli(): Promise<void> {
  try {
    cli.value = await cliPathStatus();
  } catch {
    cli.value = null;
  }
}

async function toggleCliPath(): Promise<void> {
  busy.value = "cli:path";
  try {
    if (cli.value?.installed) {
      await removeCliFromPath();
    } else {
      await installCliToPath();
    }
  } catch (e) {
    toast.error("Couldn't update the yerd CLI on PATH", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadCli();
  }
}

async function openApproval(): Promise<void> {
  try {
    await openLoginItems();
  } catch (e) {
    toast.error("Couldn't open Login Items", (e as IpcError).message);
  }
}

// ── self-update ──
// Run a check (no override → uses the saved channel). Mirror the saved channel
// the daemon reports back into the selector so it stays the single source of
// truth.
// `notify` is true only for the explicit "Check now" button — a check triggered
// any other way (channel change) updates the shown status silently, so opening
// Settings never pops an unsolicited toast.
async function runUpdateCheck(notify = false): Promise<void> {
  checkingUpdates.value = true;
  try {
    const status = await checkUpdates();
    updateStatus.value = status;
    updateChannel.value = status.channel;
    if (!notify) return;
    if (status.available && status.target) {
      toast.success("Update available", `Version ${status.target} can be installed.`);
    } else if (status.ahead_of_stable) {
      toast.info("Up to date", "You're on a pre-release ahead of stable.");
    } else if (status.source === "cached") {
      // The daemon couldn't reach the update server and served its last cached
      // result — don't claim "latest version" off possibly-stale data.
      toast.info(
        "Couldn't reach the update server",
        "Showing the last cached result — you may be offline.",
      );
    } else {
      toast.success("Up to date", "You're on the latest version.");
    }
  } catch (e) {
    if (notify) toast.error("Couldn't check for updates", (e as IpcError).message);
  } finally {
    checkingUpdates.value = false;
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

onMounted(() => {
  loadAutostart();
  hostPlatform()
    .then((p) => (platform.value = p))
    .catch(() => {});
  loadCli();
  if (running.value) loadDumps();
  // No auto update-check on open — it only runs when the user clicks "Check now".
});

// Refresh the dump-capture row whenever the daemon comes up (and clear it when
// it goes away), so the subsystem list doesn't show stale dump state.
watch(running, (up) => {
  if (up) {
    loadDumps();
  } else {
    dumps.value = null;
    updateStatus.value = null;
  }
});

// ── daemon lifecycle ──
async function onStart(): Promise<void> {
  // Spinner + on-failure diagnostics are owned by the composable; it keeps
  // spinning until the daemon connects (watch) or surfaces the panel.
  await startDaemonFlow({ nudge: true });
  // Refresh the approval banners (a fresh registration may now be pending).
  await loadAutostart();
}

async function onStop(): Promise<void> {
  busy.value = "daemon";
  try {
    await stopDaemon();
    toast.success("Stopping daemon…");
  } catch (e) {
    toast.error("Couldn't stop the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
  }
}

// ── daemon restart (confirm modal) ──
const restartDaemonOpen = ref(false);
function openRestartDaemon(): void {
  void nextTick(() => {
    restartDaemonOpen.value = true;
  });
}
async function confirmRestartDaemon(close: () => void): Promise<void> {
  busy.value = "restart:daemon";
  close();
  try {
    await restartDaemon();
    toast.info("Restarting daemon…", "It returns in a few seconds.");
  } catch (e) {
    toast.info("Restarting daemon…", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── autostart toggles ──
async function toggleDaemonLogin(on: boolean): Promise<void> {
  busy.value = "login:daemon";
  try {
    await setAutostartDaemon(on);
  } catch (e) {
    toast.error("Couldn't change daemon autostart", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}

async function toggleGuiLogin(on: boolean): Promise<void> {
  busy.value = "login:gui";
  try {
    await setAutostartGui(on);
  } catch (e) {
    toast.error("Couldn't change app autostart", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}

async function toggleGuiMinimized(on: boolean): Promise<void> {
  busy.value = "login:gui-min";
  try {
    await setAutostartGuiMinimized(on);
  } catch (e) {
    toast.error("Couldn't change the minimized option", (e as IpcError).message);
  } finally {
    busy.value = null;
    await loadAutostart();
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Settings" subtitle="Daemon, startup, and appearance" />

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <!-- macOS: registered via SMAppService but awaiting Login-Items approval. -->
      <div
        v-if="autostart?.daemonPendingApproval"
        class="flex items-start justify-between gap-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4"
      >
        <div class="space-y-1">
          <p class="text-sm font-medium">Approve the Yerd background daemon</p>
          <p class="text-xs text-muted-foreground">
            Yerd is registered but waiting for you to enable it in System Settings
            → Login Items before it can serve your .test sites.
          </p>
        </div>
        <Button variant="outline" size="sm" @click="openApproval">
          Open Login Items
        </Button>
      </div>

      <!-- macOS: the GUI login item is registered but awaiting Login-Items approval. -->
      <div
        v-if="autostart?.guiPendingApproval"
        class="flex items-start justify-between gap-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4"
      >
        <div class="space-y-1">
          <p class="text-sm font-medium">Approve launching Yerd at login</p>
          <p class="text-xs text-muted-foreground">
            “Start the Yerd app at login” is set, but macOS needs you to enable it
            under System Settings → Login Items (Open at Login).
          </p>
        </div>
        <Button variant="outline" size="sm" @click="openApproval">
          Open Login Items
        </Button>
      </div>

      <!-- Daemon + subsystems -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Daemon</CardTitle>
            <CardDescription>{{ daemonStatus }}</CardDescription>
          </div>
          <Button v-if="!running" :disabled="daemonStarting" @click="onStart">
            <Spinner v-if="daemonStarting" class="size-4" />
            <Play v-else class="size-4" /> Start
          </Button>
          <Button v-else variant="outline" :disabled="busy === 'daemon'" @click="onStop">
            <Spinner v-if="busy === 'daemon'" class="size-4" />
            <Square v-else class="size-4" /> Stop
          </Button>
        </CardHeader>
        <!-- Why a start attempt failed (only when the daemon is down). -->
        <CardContent v-if="startDiagnostics && !running">
          <DaemonDiagnosticsPanel :diagnostics="startDiagnostics" />
        </CardContent>
        <CardContent v-if="report">
          <TooltipProvider :delay-duration="0">
            <div class="text-sm">
              <div
                v-for="row in rows"
                :key="row.key"
                class="flex items-center gap-3 py-1.5"
              >
                <div
                  class="flex min-w-0 flex-1 items-center gap-1.5"
                  :class="row.child ? 'pl-3' : ''"
                >
                  <span
                    v-if="row.child"
                    class="select-none text-muted-foreground/50"
                    aria-hidden="true"
                  >
                    ↳
                  </span>
                  <span :class="row.child ? 'text-muted-foreground' : 'font-medium'">
                    {{ row.name }}
                  </span>
                  <Tooltip v-if="row.info">
                    <TooltipTrigger as-child>
                      <span class="inline-flex cursor-help text-muted-foreground">
                        <Info class="size-3.5" />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="top">{{ row.info }}</TooltipContent>
                  </Tooltip>
                </div>
                <StatusPill :tone="row.tone" :label="row.state" />
                <div class="flex w-9 shrink-0 justify-end">
                  <template v-if="row.menu">
                    <Spinner v-if="busy === 'restart:daemon'" class="size-4" />
                    <Tooltip v-else>
                      <TooltipTrigger as-child>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label="Restart daemon"
                          @click="openRestartDaemon"
                        >
                          <RotateCw class="size-4" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent side="top">Restart daemon</TooltipContent>
                    </Tooltip>
                  </template>
                </div>
              </div>
            </div>
          </TooltipProvider>
        </CardContent>
      </Card>

      <!-- Start at login -->
      <Card>
        <CardHeader>
          <CardTitle>Start at login</CardTitle>
          <CardDescription>Run Yerd automatically when you log in.</CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Run the Yerd daemon in the background</p>
              <p class="text-xs text-muted-foreground">
                {{ autostart?.daemonSupported === false
                  ? "Unavailable - no per-user service manager on this system."
                  : isMac
                    ? "Runs at login and serves your .test sites. Shows as “Yerd” in System Settings › Login Items. Use the tray Stop to stop it for this session; turn this off to keep it stopped."
                    : "Keeps your .test sites served after you log in." }}
              </p>
            </div>
            <Switch
              :model-value="autostart?.daemon ?? false"
              :disabled="busy === 'login:daemon' || autostart?.daemonSupported === false"
              aria-label="Run the Yerd daemon in the background"
              @update:model-value="toggleDaemonLogin"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Start the Yerd app at login</p>
              <p class="text-xs text-muted-foreground">Open this window when you log in.</p>
            </div>
            <Switch
              :model-value="autostart?.gui ?? false"
              :disabled="busy === 'login:gui'"
              aria-label="Start the Yerd app at login"
              @update:model-value="toggleGuiLogin"
            />
          </div>

          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Start the Yerd app minimized</p>
              <p class="text-xs text-muted-foreground">Launch hidden to the tray instead of showing the window.</p>
            </div>
            <Switch
              :model-value="autostart?.guiMinimized ?? false"
              :disabled="busy === 'login:gui-min' || !autostart?.gui"
              aria-label="Start the Yerd app minimized"
              @update:model-value="toggleGuiMinimized"
            />
          </div>
        </CardContent>
      </Card>

      <!-- Terminal CLI (macOS) — Linux exposes `yerd` on PATH via the .deb. -->
      <Card v-if="isMac">
        <CardHeader>
          <CardTitle>Terminal CLI</CardTitle>
          <CardDescription>Use the <code>yerd</code> command in your terminal.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Install <code>yerd</code> on your PATH</p>
              <p class="text-xs text-muted-foreground">
                {{ cli?.installed
                  ? "Installed - run `yerd` in a new terminal window."
                  : "Symlinks the bundled CLI into your shell PATH." }}
              </p>
            </div>
            <Button
              variant="outline"
              size="sm"
              :disabled="busy === 'cli:path'"
              @click="toggleCliPath"
            >
              <Spinner v-if="busy === 'cli:path'" class="size-4" />
              {{ cli?.installed ? "Remove" : "Install" }}
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Appearance -->
      <Card>
        <CardHeader>
          <CardTitle>Appearance</CardTitle>
          <CardDescription>Theme used by the Yerd app.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Theme</p>
              <p class="text-xs text-muted-foreground">
                Match your system, or force light or dark.
              </p>
            </div>
            <Select
              :model-value="pref"
              :options="themeOptions"
              aria-label="Theme"
              @update:model-value="(v: ThemePref) => setTheme(v)"
            />
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

          <div class="flex items-center justify-between gap-4 border-t pt-4">
            <div class="min-w-0 text-sm">
              <template v-if="updateStatus">
                <p class="font-medium">{{ updateSummary }}</p>
                <p class="text-xs text-muted-foreground">
                  Current {{ updateStatus.current }} · stable
                  {{ updateStatus.latest_stable ?? "-" }} · edge
                  {{ updateStatus.latest_edge ?? "-" }}
                  <span v-if="updateStatus.source === 'cached'">
                    · offline (last known)
                  </span>
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
    </div>

    <Modal v-model:open="restartDaemonOpen" title="Restart daemon">
      <p class="text-sm text-muted-foreground">
        This briefly stops all <strong class="text-foreground">.test</strong> sites,
        DNS, and this connection while the daemon restarts. It returns in a few
        seconds.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmRestartDaemon(close)">Restart</Button>
      </template>
    </Modal>
  </div>
</template>
