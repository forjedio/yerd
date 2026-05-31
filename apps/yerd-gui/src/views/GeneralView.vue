<script setup lang="ts">
import {
  CircleDot,
  CircleOff,
  FileText,
  Info,
  MoreHorizontal,
  Network,
  Play,
  RotateCw,
  Square,
  Undo2,
} from "lucide-vue-next";
import { computed, nextTick, onMounted, ref, watch } from "vue";

import ComingSoon from "@/components/ComingSoon.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
  elevate,
  elevateAll,
  getAutostart,
  hostPlatform,
  IpcError,
  restartDaemon,
  setAutostartDaemon,
  setAutostartGui,
  setAutostartGuiMinimized,
  startDaemon,
  stopDaemon,
  trustCa,
  unelevate,
  untrustCa,
} from "@/ipc/client";
import type { AutostartState, ElevateTarget, StatusReport } from "@/ipc/types";
import { useTheme, type ThemePref } from "@/lib/theme";
import { humaniseBytes, humaniseUptime } from "@/lib/utils";

const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();
const { pref, setTheme } = useTheme();

const busy = ref<string | null>(null);
const autostart = ref<AutostartState | null>(null);
const platform = ref<string>("");
// macOS only: set true when a GUI untrust left a system-wide trust (set via
// `sudo yerd elevate trust`) in place — the GUI can't remove that without root.
// Drives the trust row to hide the (now-useless) Revert button and show guidance.
// Cleared once the CA actually reads not-trusted again (e.g. after the user runs
// `sudo yerd unelevate trust`) — see the watcher below.
const systemTrustRemains = ref(false);
const canElevate = computed(
  () => platform.value === "linux" || platform.value === "macos",
);

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
  memory: string;
  info: string;
  child?: boolean; // indented under the daemon (runs inside its process)
  menu?: boolean; // the daemon row gets a Restart/Logs menu
}

const PRIVILEGED_PORT_CEILING = 1024;

function privilegedFallback(r: StatusReport): boolean {
  return (
    (r.http.requested < PRIVILEGED_PORT_CEILING && r.http.fell_back) ||
    (r.https.requested < PRIVILEGED_PORT_CEILING && r.https.fell_back)
  );
}

function portsElevated(r: StatusReport): boolean {
  return !privilegedFallback(r) || r.port_redirect === true;
}

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
    memory: "—",
    info: `${detail} · runs inside the daemon process`,
    child: true,
  };
}

const rows = computed<Row[]>(() => {
  const r = report.value;
  if (!r) return [];
  return [
    {
      key: "daemon",
      name: "Daemon (yerdd)",
      tone: "ok",
      state: "running",
      memory: humaniseBytes(r.daemon_rss_bytes),
      info: `pid ${r.daemon_pid} · up ${humaniseUptime(r.uptime_secs)}`,
      menu: true,
    },
    {
      key: "dns",
      name: "DNS resolver",
      tone: "ok",
      state: "listening",
      memory: "—",
      info: `bound on ${r.dns_addr} · runs inside the daemon process`,
      child: true,
    },
    portRow("proxy-http", "Proxy (HTTP)", r, "http"),
    portRow("proxy-https", "Proxy (HTTPS)", r, "https"),
  ];
});

// ── environment (tri-state OS privileges) ──
type Tri = boolean | null;
function triTone(v: Tri): Tone {
  return v === true ? "ok" : v === false ? "bad" : "unknown";
}
function triLabel(v: Tri, yes: string, no: string): string {
  return v === true ? yes : v === false ? no : "unknown";
}

interface EnvItem {
  key: string;
  label: string;
  value: Tri;
  fixable: boolean;
  // Shown only when *Yerd* established this privilege (so the undo is meaningful).
  // Never derived from `portsElevated` (true even when Yerd installed nothing).
  // Ports is reversible on macOS only (Linux setcap has no clean reverse).
  unelevatable: boolean;
  target: ElevateTarget;
  yes: string;
  no: string;
  // Optional inline guidance shown in the action cell (e.g. when a residual
  // system-wide trust can only be removed from a terminal).
  note?: string;
}
const envItems = computed<EnvItem[]>(() => {
  const r = report.value;
  if (!r) return [];
  const mac = platform.value === "macos";
  return [
    {
      key: "trust",
      label: "Local CA trusted",
      value: r.ca.trusted_system,
      fixable: r.ca.trusted_system !== true,
      // Hide Revert when a system-wide trust remains that the GUI can't undo —
      // a lingering Revert there does nothing and looks broken.
      unelevatable: r.ca.trusted_system === true && !(mac && systemTrustRemains.value),
      target: "trust",
      yes: "trusted",
      no: "not trusted",
      note:
        mac && systemTrustRemains.value
          ? "Trusted system-wide (via Terminal) — run `sudo yerd unelevate trust` to remove it."
          : undefined,
    },
    {
      key: "resolver",
      label: ".test resolver installed",
      value: r.resolver_installed,
      fixable: r.resolver_installed !== true,
      unelevatable: r.resolver_installed === true,
      target: "resolver",
      yes: "installed",
      no: "not installed",
    },
    {
      key: "ports",
      label: "Privileged ports (80/443)",
      value: portsElevated(r),
      fixable: !portsElevated(r),
      unelevatable: r.port_redirect === true && mac,
      target: "ports",
      yes: r.port_redirect === true && privilegedFallback(r) ? "redirected" : "bound",
      no: "fell back to high ports",
    },
  ];
});

// ── data loads ──
async function loadAutostart(): Promise<void> {
  try {
    autostart.value = await getAutostart();
  } catch (e) {
    toast.error("Couldn't load startup settings", (e as IpcError).message);
  }
}

onMounted(() => {
  void loadAutostart();
  void hostPlatform().then((p) => (platform.value = p));
});

// Once the CA actually reads not-trusted again (e.g. the user removed the
// system-wide trust via `sudo yerd unelevate trust`), drop the residual flag so
// the trust row returns to its normal Fix/Revert behaviour.
watch(
  () => report.value?.ca.trusted_system,
  (trusted) => {
    if (trusted !== true) systemTrustRemains.value = false;
  },
);

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

// ── elevation ──
// Anything still needing a privileged fix? Gates the "Fix all" button.
const anyFixable = computed(() => envItems.value.some((i) => i.fixable));

async function onFixAll(): Promise<void> {
  busy.value = "elevate:all";
  try {
    if (platform.value === "macos") {
      // On macOS, CA trust is in-process (user domain, no root); only
      // resolver + ports go through osascript. Run each step independently so
      // one cancellation doesn't report the others as failed.
      const steps: { label: string; run: () => Promise<unknown> }[] = [
        { label: "trust", run: () => onTrustCa() },
        { label: "resolver", run: () => elevate("resolver") },
        { label: "ports", run: () => elevate("ports") },
      ];
      const failed: string[] = [];
      for (const s of steps) {
        try {
          await s.run();
        } catch (e) {
          failed.push(`${s.label}: ${(e as IpcError).message}`);
        }
      }
      if (failed.length === 0) {
        toast.success("Privileges granted", "You may be prompted by the OS.");
      } else if (failed.length === steps.length) {
        toast.error("Elevation failed", failed.join("; "));
      } else {
        toast.info("Partly done", `Couldn't complete — ${failed.join("; ")}`);
      }
    } else {
      await elevateAll();
      toast.success("Privileges granted", "You may be prompted by the OS.");
    }
  } catch (e) {
    toast.error("Elevation failed", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
  }
}

/** Trust the CA: macOS uses the in-process user-domain path; elsewhere the CLI. */
async function onTrustCa(): Promise<void> {
  if (platform.value === "macos") {
    await trustCa();
    systemTrustRemains.value = false;
  } else {
    await elevate("trust");
  }
}

async function onElevate(target: ElevateTarget): Promise<void> {
  busy.value = `elevate:${target}`;
  try {
    if (target === "trust") {
      await onTrustCa();
    } else {
      await elevate(target);
    }
    toast.success("Privilege granted", "You may be prompted by the OS.");
  } catch (e) {
    toast.error("Elevation failed", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
  }
}

const unelevateOpen = ref(false);
const pendingUnelevate = ref<ElevateTarget | null>(null);
const UNELEVATE_COPY: Record<ElevateTarget, { title: string; body: string }> = {
  trust: {
    title: "Untrust local CA",
    body: "Removes Yerd's local CA trust for your user account. HTTPS .test sites will show certificate warnings until you trust it again. (A trust set system-wide via `sudo yerd elevate trust` must be removed with `sudo yerd unelevate trust`.)",
  },
  resolver: {
    title: "Remove .test resolver",
    body: "Removes Yerd's .test resolver. If a previous resolver was backed up when Yerd was set up, it's restored automatically; otherwise .test names stop resolving through Yerd.",
  },
  ports: {
    title: "Remove port redirect",
    body: "Removes the 80/443 → Yerd redirect. Sites stay reachable on Yerd's high ports until you re-enable it.",
  },
};
const unelevateCopy = computed(() =>
  pendingUnelevate.value ? UNELEVATE_COPY[pendingUnelevate.value] : null,
);
function openUnelevate(target: ElevateTarget): void {
  pendingUnelevate.value = target;
  unelevateOpen.value = true;
}
async function confirmUnelevate(close: () => void): Promise<void> {
  const target = pendingUnelevate.value;
  if (!target) return;
  busy.value = `unelevate:${target}`;
  close();
  try {
    if (target === "trust" && platform.value === "macos") {
      const residual = await untrustCa();
      systemTrustRemains.value = residual;
      if (residual) {
        toast.info(
          "Removed for your user",
          "A system-wide trust set via Terminal remains — run `sudo yerd unelevate trust` to remove it.",
        );
      } else {
        toast.success("Reverted");
      }
    } else {
      await unelevate(target);
      toast.success("Reverted", "You may be prompted by the OS.");
    }
  } catch (e) {
    toast.error("Couldn't revert", (e as IpcError).message);
  } finally {
    busy.value = null;
    pendingUnelevate.value = null;
    await refreshStatus();
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
    <PageHeader title="General" subtitle="Daemon, environment, startup, and appearance" />

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
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                  <th class="py-2 pr-4 font-medium">Subsystem</th>
                  <th class="py-2 pr-4 font-medium">Status</th>
                  <th class="py-2 pr-4 font-medium">Memory</th>
                  <th class="py-2 pl-4 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="row in rows" :key="row.key" class="border-b last:border-0">
                  <td class="py-3 pr-4 font-medium">
                    <div class="flex items-center gap-1.5" :class="row.child ? 'pl-5' : ''">
                      <span v-if="row.child" class="text-muted-foreground">↳</span>
                      <span>{{ row.name }}</span>
                      <Tooltip v-if="row.info">
                        <TooltipTrigger as-child>
                          <span class="inline-flex cursor-help text-muted-foreground">
                            <Info class="size-3.5" />
                          </span>
                        </TooltipTrigger>
                        <TooltipContent side="top">{{ row.info }}</TooltipContent>
                      </Tooltip>
                    </div>
                  </td>
                  <td class="py-3 pr-4">
                    <StatusPill :tone="row.tone" :label="row.state" />
                  </td>
                  <td class="py-3 pr-4 text-xs text-muted-foreground">{{ row.memory }}</td>
                  <td class="py-3 pl-4">
                    <div class="flex items-center justify-end">
                      <Spinner v-if="row.menu && busy === 'restart:daemon'" class="size-4" />
                      <DropdownMenu v-if="row.menu">
                        <DropdownMenuTrigger as-child>
                          <Button variant="ghost" size="icon" :aria-label="`Actions for ${row.name}`">
                            <MoreHorizontal class="size-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem @select="openRestartDaemon">
                            <RotateCw class="size-4" /> Restart
                          </DropdownMenuItem>
                          <DropdownMenuItem disabled>
                            <FileText class="size-4" /> Logs (soon)
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </TooltipProvider>
        </CardContent>
      </Card>

      <!-- Environment (OS-level privileges) -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle class="flex items-center gap-2"><Network class="size-4" /> Environment</CardTitle>
            <CardDescription>
              OS-level trust, resolver, and privileged-port configuration.
            </CardDescription>
          </div>
          <Button
            v-if="canElevate"
            size="sm"
            :disabled="!anyFixable || busy === 'elevate:all'"
            @click="onFixAll"
          >
            <Spinner v-if="busy === 'elevate:all'" class="size-4" />
            Fix all (elevate)
          </Button>
        </CardHeader>
        <CardContent>
          <p
            v-if="!report && connected === false"
            class="py-6 text-center text-sm text-muted-foreground"
          >
            Start the daemon to view and change OS privileges.
          </p>
          <div v-else-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <table v-else class="w-full text-sm">
            <tbody>
              <tr v-for="item in envItems" :key="item.key" class="border-b last:border-0">
                <td class="py-3 pr-4 font-medium">{{ item.label }}</td>
                <td class="py-3 pr-4">
                  <StatusPill :tone="triTone(item.value)" :label="triLabel(item.value, item.yes, item.no)" />
                </td>
                <td class="py-3 pl-4 text-right">
                  <template v-if="item.fixable">
                    <Button
                      v-if="canElevate"
                      variant="outline"
                      size="sm"
                      :disabled="busy === `elevate:${item.target}`"
                      @click="onElevate(item.target)"
                    >
                      <Spinner v-if="busy === `elevate:${item.target}`" class="size-4" />
                      Fix (elevate)
                    </Button>
                    <ComingSoon
                      v-else
                      reason="In-app elevation isn't available on this platform yet — use `yerd elevate` in a terminal for now."
                      pill
                    >
                      Fix
                    </ComingSoon>
                  </template>
                  <Button
                    v-else-if="item.unelevatable && canElevate"
                    variant="ghost"
                    size="sm"
                    :disabled="busy === `unelevate:${item.target}`"
                    @click="openUnelevate(item.target)"
                  >
                    <Spinner v-if="busy === `unelevate:${item.target}`" class="size-4" />
                    <Undo2 v-else class="size-4" />
                    Revert
                  </Button>
                  <span
                    v-else-if="item.note"
                    class="text-xs text-muted-foreground"
                  >{{ item.note }}</span>
                </td>
              </tr>
            </tbody>
          </table>
          <p v-if="report" class="mt-3 text-xs text-muted-foreground">
            Fixes run the audited <code>yerd elevate</code> helper under an OS
            prompt; the GUI itself never runs as root.
          </p>
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
                  ? "Unavailable — no per-user service manager on this system."
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
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Appearance</CardTitle>
            <CardDescription>Theme used by the Yerd app.</CardDescription>
          </div>
          <Select
            :model-value="pref"
            :options="themeOptions"
            aria-label="Theme"
            @update:model-value="(v: ThemePref) => setTheme(v)"
          />
        </CardHeader>
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

    <Modal v-model:open="unelevateOpen" :title="unelevateCopy?.title ?? 'Revert privilege'">
      <p class="text-sm text-muted-foreground">{{ unelevateCopy?.body }}</p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmUnelevate(close)">Revert</Button>
      </template>
    </Modal>
  </div>
</template>
