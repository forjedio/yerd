<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  Copy,
  Network,
  Power,
  RefreshCw,
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
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  diagnose,
  doctorFix,
  elevate,
  hostPlatform,
  IpcError,
} from "@/ipc/client";
import type {
  Diagnosis,
  ElevateTarget,
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
const canElevate = computed(() => platform.value === "linux");

const diagnoses = ref<Diagnosis[]>([]);
const diagLoading = ref(true);
const busy = ref<string | null>(null);

// ── derived subsystem rows ──
interface Row {
  key: string;
  name: string;
  tone: Tone;
  state: string;
  detail: string;
  failedFpm?: string; // version when an FPM pool has failed
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
      detail: `pid ${r.daemon_pid} · up ${humaniseUptime(r.uptime_secs)}`,
    },
    {
      key: "dns",
      name: "DNS resolver",
      tone: "ok",
      state: "listening",
      detail: r.dns_addr,
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
      detail: [p.pid ? `pid ${p.pid}` : null, p.listen, humaniseBytes(p.rss_bytes)]
        .filter(Boolean)
        .join(" · "),
      failedFpm: p.state === "failed" ? p.version : undefined,
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
  return {
    key,
    name,
    tone: ps.fell_back ? "warn" : "ok",
    state: `:${ps.bound}`,
    detail: ps.fell_back ? `rootless fallback from :${ps.requested}` : "privileged port",
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

// ── actions ──
async function loadDiagnoses(): Promise<void> {
  diagLoading.value = true;
  try {
    diagnoses.value = await diagnose();
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

async function restartFailedFpm(): Promise<void> {
  // doctor fix is the only restart path that exists; it restarts failed pools.
  await runFixes();
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
    <PageHeader title="Services" subtitle="Yerd's own subsystems, health, and environment">
      <template #actions>
        <ComingSoon reason="Restarting the daemon needs a daemon Restart/Shutdown IPC — coming soon.">
          <Power class="size-4" /> Restart daemon
        </ComingSoon>
      </template>
    </PageHeader>

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <!-- Subsystems -->
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2"><Server class="size-4" /> Subsystems</CardTitle>
        </CardHeader>
        <CardContent>
          <div v-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <table v-else class="w-full text-sm">
            <tbody>
              <tr v-for="row in rows" :key="row.key" class="border-b last:border-0">
                <td class="py-3 pr-4 font-medium">{{ row.name }}</td>
                <td class="py-3 pr-4">
                  <StatusPill :tone="row.tone" :label="row.state" />
                </td>
                <td class="py-3 pr-4 text-xs text-muted-foreground">{{ row.detail }}</td>
                <td class="py-3 pl-4 text-right">
                  <div class="flex items-center justify-end gap-2">
                    <Button
                      v-if="row.failedFpm"
                      variant="outline"
                      size="sm"
                      :disabled="busy === 'fix'"
                      @click="restartFailedFpm"
                    >
                      <Spinner v-if="busy === 'fix'" class="size-4" />
                      <Wrench v-else class="size-4" /> Run fix
                    </Button>
                    <ComingSoon reason="Per-service restart needs a daemon IPC — coming soon." pill>
                      Restart
                    </ComingSoon>
                    <ComingSoon reason="Log viewing needs a daemon Logs IPC + file logging — coming soon." pill>
                      Logs
                    </ComingSoon>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>

      <!-- Health -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <CardTitle class="flex items-center gap-2"><ShieldCheck class="size-4" /> Health</CardTitle>
          <div class="flex gap-2">
            <Button variant="ghost" size="sm" :disabled="diagLoading" @click="loadDiagnoses">
              <RefreshCw class="size-4" /> Re-check
            </Button>
            <Button size="sm" :disabled="busy === 'fix'" @click="runFixes">
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
                      reason="macOS in-app elevation is pending a CLI socket-path fix — use `yerd elevate` in a terminal for now."
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
  </div>
</template>
