<script setup lang="ts">
import {
  CircleDot,
  CircleOff,
  Info,
  Play,
  RotateCw,
  Square,
} from "lucide-vue-next";
import { computed, nextTick, onMounted, ref, watch } from "vue";

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
import { useToast } from "@/composables/useToast";
import {
  dumpsStatus,
  getAutostart,
  IpcError,
  restartDaemon,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
  startDaemon,
  stopDaemon,
} from "@/ipc/client";
import type {
  AutostartState,
  DumpsStatusResponse,
  StatusReport,
} from "@/ipc/types";
import { useTheme, type ThemePref } from "@/lib/theme";
import { humaniseBytes, humaniseUptime } from "@/lib/utils";

const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();
const { pref, setTheme } = useTheme();

const busy = ref<string | null>(null);
const autostart = ref<AutostartState | null>(null);
// Dump-capture status (separate IPC from the report) — feeds the subsystem list.
const dumps = ref<DumpsStatusResponse | null>(null);

const themeOptions = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
] as const;

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

onMounted(() => {
  loadAutostart();
  if (running.value) loadDumps();
});

// Refresh the dump-capture row whenever the daemon comes up (and clear it when
// it goes away), so the subsystem list doesn't show stale dump state.
watch(running, (up) => {
  if (up) loadDumps();
  else dumps.value = null;
});

// ── daemon lifecycle ──
async function onStart(): Promise<void> {
  busy.value = "daemon";
  try {
    await startDaemon();
    toast.success("Starting daemon…", "It should connect in a moment.");
  } catch (e) {
    toast.error("Couldn't start the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
  }
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
      <!-- Daemon + subsystems -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle class="flex items-center gap-2">
              <CircleDot v-if="running" class="size-4 text-success" />
              <CircleOff v-else class="size-4 text-muted-foreground" />
              Daemon
            </CardTitle>
            <CardDescription>{{ daemonStatus }}</CardDescription>
          </div>
          <Button v-if="!running" :disabled="busy === 'daemon'" @click="onStart">
            <Spinner v-if="busy === 'daemon'" class="size-4" />
            <Play v-else class="size-4" /> Start
          </Button>
          <Button v-else variant="outline" :disabled="busy === 'daemon'" @click="onStop">
            <Spinner v-if="busy === 'daemon'" class="size-4" />
            <Square v-else class="size-4" /> Stop
          </Button>
        </CardHeader>
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
              <p class="text-sm font-medium">Start the Yerd daemon at login</p>
              <p class="text-xs text-muted-foreground">
                {{ autostart?.daemonSupported === false
                  ? "Unavailable - no per-user service manager on this system."
                  : "Keeps your .test sites served after you log in." }}
              </p>
            </div>
            <Switch
              :model-value="autostart?.daemon ?? false"
              :disabled="busy === 'login:daemon' || autostart?.daemonSupported === false"
              aria-label="Start the Yerd daemon at login"
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
