<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import {
  Copy,
  FileText,
  Info,
  MoreHorizontal,
  Network,
  RefreshCw,
  RotateCw,
  Server,
  ShieldCheck,
  Wrench,
} from "lucide-vue-next";

import ComingSoon from "@/components/ComingSoon.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Badge from "@/components/ui/Badge.vue";
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
import Spinner from "@/components/ui/Spinner.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  diagnose,
  doctorFix,
  elevate,
  hostPlatform,
  IpcError,
  restartDaemon,
  restartPhp,
} from "@/ipc/client";
import type {
  Diagnosis,
  ElevateTarget,
  PhpVersion,
  Severity,
  StatusReport,
} from "@/ipc/types";
import {
  humaniseBytes,
  humaniseUptime,
  poolStateLabel,
  poolStateTone,
} from "@/lib/utils";

const toast = useToast();
const { report, refresh: refreshStatus } = useDaemon();

const platform = ref<string>("");
const canElevate = computed(
  () => platform.value === "linux" || platform.value === "macos",
);

const diagnoses = ref<Diagnosis[]>([]);
const diagLoading = ref(true);
const busy = ref<string | null>(null);

// ── derived subsystem rows ──
interface Row {
  key: string;
  name: string;
  tone: Tone;
  state: string;
  memory: string; // 3rd column (RAM); "—" for in-process subsystems
  info: string; // details shown in the (i) tooltip
  child?: boolean; // indented under the daemon (runs inside its process)
  // Which actions the row's ⋯ menu offers. DNS/proxy have none (no menu).
  menu?: "daemon" | "fpm";
  version?: PhpVersion; // for fpm restart
  canRestart?: boolean; // fpm: only when running or failed (not idle)
}

const rows = computed<Row[]>(() => {
  const r = report.value;
  if (!r) return [];
  const list: Row[] = [
    {
      key: "daemon",
      name: "Daemon (yerdd)",
      tone: "ok",
      state: "running",
      memory: humaniseBytes(r.daemon_rss_bytes),
      info: `pid ${r.daemon_pid} · up ${humaniseUptime(r.uptime_secs)}`,
      menu: "daemon",
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
  for (const p of r.php) {
    list.push({
      key: `fpm-${p.version}`,
      name: `PHP-FPM ${p.version}`,
      tone: poolStateTone(p.state),
      state: poolStateLabel(p.state),
      memory: humaniseBytes(p.rss_bytes),
      // Empty when idle (no pid/listen) → the (i) icon is then omitted.
      info: [p.pid ? `pid ${p.pid}` : null, p.listen].filter(Boolean).join(" · "),
      menu: "fpm",
      version: p.version,
      canRestart: p.state === "running" || p.state === "failed",
    });
  }
  return list;
});

function portRow(
  key: string,
  name: string,
  r: StatusReport,
  which: "http" | "https",
): Row {
  const ps = r[which];
  const port = ps.fell_back ? `rootless fallback from :${ps.requested}` : "privileged port";
  return {
    key,
    name,
    tone: ps.fell_back ? "warn" : "ok",
    state: `:${ps.bound}`,
    memory: "—",
    info: `${port} · runs inside the daemon process`,
    child: true,
  };
}

// ── environment (tri-state) ──
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
  target: ElevateTarget;
  yes: string;
  no: string;
}
const envItems = computed<EnvItem[]>(() => {
  const r = report.value;
  if (!r) return [];
  return [
    {
      key: "trust",
      label: "Local CA trusted",
      value: r.ca.trusted_system,
      fixable: r.ca.trusted_system !== true,
      target: "trust",
      yes: "trusted",
      no: "not trusted",
    },
    {
      key: "resolver",
      label: ".test resolver installed",
      value: r.resolver_installed,
      fixable: r.resolver_installed !== true,
      target: "resolver",
      yes: "installed",
      no: "not installed",
    },
    {
      key: "ports",
      label: "Privileged ports (80/443)",
      value: r.http.fell_back || r.https.fell_back ? false : true,
      fixable: r.http.fell_back || r.https.fell_back,
      target: "ports",
      yes: "bound",
      no: "fell back to high ports",
    },
  ];
});

// Whether there's anything worth fixing — "Run safe fixes" is only enabled
// when at least one diagnosis is a warning or failure.
const hasActionable = computed(() =>
  diagnoses.value.some((d) => d.severity === "warn" || d.severity === "fail"),
);

// ── actions ──
async function loadDiagnoses(notify = false): Promise<void> {
  diagLoading.value = true;
  try {
    diagnoses.value = await diagnose();
    if (notify) toast.success("Health re-checked");
  } catch (e) {
    toast.error("Couldn't run diagnostics", (e as IpcError).message);
  } finally {
    diagLoading.value = false;
  }
}

async function runFixes(): Promise<void> {
  busy.value = "fix";
  try {
    const r = await doctorFix();
    const ok = r.performed.filter((p) => p.ok).length;
    toast.success(
      "Ran safe fixes",
      `${ok}/${r.performed.length} applied · ${r.manual.length} need manual action`,
    );
    await Promise.all([loadDiagnoses(), refreshStatus()]);
  } catch (e) {
    toast.error("Fix run failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doRestartFpm(v: PhpVersion): Promise<void> {
  busy.value = `restart:${v}`;
  try {
    await restartPhp(v);
    toast.success(`Restarted PHP-FPM ${v}`);
    await refreshStatus();
  } catch (e) {
    toast.error(`Couldn't restart PHP-FPM ${v}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── daemon restart (confirm modal) ──
const restartDaemonOpen = ref(false);

// Defer opening past the dropdown's close so reka-ui's focus-restore doesn't
// steal focus from the modal.
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
    // The daemon flushes Ok before re-execing, so the happy path resolves; a
    // dropped connection here just means it tore down — treat softly.
    toast.info("Restarting daemon…", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function onElevate(target: ElevateTarget): Promise<void> {
  busy.value = `elevate:${target}`;
  try {
    await elevate(target);
    toast.success("Privilege granted", "You may be prompted by the OS.");
    await refreshStatus();
    await loadDiagnoses();
  } catch (e) {
    toast.error("Elevation failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function copyRemedy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast.info("Copied to clipboard");
  } catch {
    toast.error("Couldn't copy");
  }
}

const sevVariant: Record<Severity, "success" | "warning" | "destructive"> = {
  ok: "success",
  warn: "warning",
  fail: "destructive",
};

onMounted(() => {
  void loadDiagnoses();
  void hostPlatform().then((p) => (platform.value = p));
});
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Services" subtitle="Yerd's own subsystems, health, and environment" />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <!-- Subsystems -->
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2"><Server class="size-4" /> Subsystems</CardTitle>
          <CardDescription>
            The daemon and the proxy, DNS, and PHP-FPM processes it runs.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div v-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <TooltipProvider v-else :delay-duration="0">
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
                  <div
                    class="flex items-center gap-1.5"
                    :class="row.child ? 'pl-5' : ''"
                  >
                    <span v-if="row.child" class="text-muted-foreground">↳</span>
                    <span>{{ row.name }}</span>
                    <Tooltip v-if="row.info">
                      <TooltipTrigger as-child>
                        <span class="inline-flex cursor-help text-muted-foreground">
                          <Info class="size-3.5" />
                        </span>
                      </TooltipTrigger>
                      <TooltipContent side="top" class="w-72">{{ row.info }}</TooltipContent>
                    </Tooltip>
                  </div>
                </td>
                <td class="py-3 pr-4">
                  <StatusPill :tone="row.tone" :label="row.state" />
                </td>
                <td class="py-3 pr-4 text-xs text-muted-foreground">{{ row.memory }}</td>
                <td class="py-3 pl-4">
                  <div class="flex items-center justify-end">
                    <Spinner v-if="busy?.endsWith(`:${row.version ?? 'daemon'}`)" class="size-4" />
                    <DropdownMenu v-if="row.menu">
                      <DropdownMenuTrigger as-child>
                        <Button variant="ghost" size="icon" :aria-label="`Actions for ${row.name}`">
                          <MoreHorizontal class="size-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem
                          v-if="row.menu === 'daemon'"
                          @select="openRestartDaemon"
                        >
                          <RotateCw class="size-4" /> Restart
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          v-else
                          :disabled="!row.canRestart"
                          @select="row.version && doRestartFpm(row.version)"
                        >
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

      <!-- Health -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle class="flex items-center gap-2"><ShieldCheck class="size-4" /> Health</CardTitle>
            <CardDescription>Common problems and safe one-click fixes.</CardDescription>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              :disabled="diagLoading"
              aria-label="Re-check health"
              @click="loadDiagnoses(true)"
            >
              <Spinner v-if="diagLoading" class="size-4" />
              <RefreshCw v-else class="size-4" />
            </Button>
            <Button size="sm" :disabled="!hasActionable || busy === 'fix'" @click="runFixes">
              <Spinner v-if="busy === 'fix'" class="size-4" />
              <Wrench v-else class="size-4" /> Run safe fixes
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <div v-if="diagLoading" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <ul v-else class="space-y-3">
            <li
              v-for="(d, i) in diagnoses"
              :key="i"
              class="flex items-start gap-3 rounded-md border p-3"
            >
              <Badge :variant="sevVariant[d.severity]" class="mt-0.5 shrink-0">{{ d.severity }}</Badge>
              <div class="min-w-0 flex-1">
                <p class="text-sm font-medium">{{ d.title }}</p>
                <p class="text-xs text-muted-foreground">{{ d.detail }}</p>
                <div
                  v-if="d.remedy"
                  class="mt-2 flex items-center gap-2 rounded bg-muted px-2 py-1 font-mono text-xs"
                >
                  <span class="min-w-0 flex-1 truncate">{{ d.remedy }}</span>
                  <button class="text-muted-foreground hover:text-foreground" @click="copyRemedy(d.remedy!)">
                    <Copy class="size-3.5" />
                  </button>
                </div>
              </div>
            </li>
          </ul>
        </CardContent>
      </Card>

      <!-- Environment -->
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2"><Network class="size-4" /> Environment</CardTitle>
          <CardDescription>
            OS-level trust, resolver, and privileged-port configuration.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div v-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <table v-else class="w-full text-sm">
            <tbody>
              <tr v-for="item in envItems" :key="item.key" class="border-b last:border-0">
                <td class="py-3 pr-4 font-medium">{{ item.label }}</td>
                <td class="py-3 pr-4">
                  <StatusPill
                    :tone="triTone(item.value)"
                    :label="triLabel(item.value, item.yes, item.no)"
                  />
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
                </td>
              </tr>
            </tbody>
          </table>
          <p class="mt-3 text-xs text-muted-foreground">
            Fixes run the audited <code>yerd elevate</code> helper under an OS
            prompt; the GUI itself never runs as root.
          </p>
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
