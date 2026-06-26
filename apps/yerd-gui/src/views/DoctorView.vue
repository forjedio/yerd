<script setup lang="ts">
import { CheckCircle2, Copy, Info, Play, RefreshCw, RotateCw, Square, Wrench } from "lucide-vue-next";
import { computed, nextTick, onMounted, ref } from "vue";

import DaemonDiagnosticsPanel from "@/components/DaemonDiagnosticsPanel.vue";
import EnvironmentCard from "@/components/EnvironmentCard.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useDaemon } from "@/composables/useDaemon";
import { useDaemonStart } from "@/composables/useDaemonStart";
import { useToast } from "@/composables/useToast";
import { diagnose, doctorFix, IpcError, restartDaemon, stopDaemon } from "@/ipc/client";
import type { Diagnosis, Severity } from "@/ipc/types";

const toast = useToast();
const { connected, report, refresh: refreshStatus } = useDaemon();

// ── daemon lifecycle (moved here from Settings) ──
const {
  starting: daemonStarting,
  activeLabel: daemonStartLabel,
  diagnostics: startDiagnostics,
  start: startDaemonFlow,
} = useDaemonStart();
const busy = ref<string | null>(null);
const running = computed(() => connected.value === true);
const daemonPid = computed(() => report.value?.daemon_pid ?? null);

async function onStart(): Promise<void> {
  // Spinner + on-failure diagnostics are owned by the composable; it keeps
  // spinning until the daemon connects (watch) or surfaces the panel.
  await startDaemonFlow({ nudge: true });
  await refreshStatus();
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

const diagnoses = ref<Diagnosis[]>([]);
const diagLoading = ref(true);
const diagError = ref(false);
const fixing = ref(false);

// "Run safe fixes" is only enabled when at least one finding is a warning/failure.
const hasActionable = computed(() =>
  diagnoses.value.some((d) => d.severity === "warn" || d.severity === "fail"),
);

const sevVariant: Record<Severity, "success" | "warning" | "destructive"> = {
  ok: "success",
  warn: "warning",
  fail: "destructive",
};

// Human labels - the wire uses bare enum tokens (ok/warn/fail) that read as
// unfinished in the UI.
const sevLabel: Record<Severity, string> = {
  ok: "Healthy",
  warn: "Warning",
  fail: "Problem",
};

// Nothing to fix: either no findings, or every finding is informational/ok.
// Show positive confirmation instead of a bare list (or a blank card).
const allClear = computed(
  () =>
    diagnoses.value.length === 0 ||
    diagnoses.value.every((d) => d.severity === "ok"),
);

async function loadDiagnoses(notify = false): Promise<void> {
  diagLoading.value = true;
  diagError.value = false;
  try {
    diagnoses.value = await diagnose();
    if (notify) toast.success("Health re-checked");
  } catch (e) {
    diagError.value = true;
    toast.error("Couldn't run diagnostics", (e as IpcError).message);
  } finally {
    diagLoading.value = false;
  }
}

async function runFixes(): Promise<void> {
  fixing.value = true;
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
    fixing.value = false;
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

onMounted(() => void loadDiagnoses());
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <PageHeader title="Doctor" subtitle="Health checks and safe one-click fixes" />

    <div class="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
      <!-- Daemon control -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="flex items-center gap-1.5">
            <CardTitle>Daemon</CardTitle>
            <TooltipProvider v-if="running && daemonPid" :delay-duration="0">
              <Tooltip>
                <TooltipTrigger as-child>
                  <span class="inline-flex cursor-help text-muted-foreground">
                    <Info class="size-3.5" />
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">running as pid {{ daemonPid }}</TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </div>
          <StatusPill
            :tone="running ? 'ok' : 'bad'"
            :label="running ? 'Running' : 'Stopped'"
          />
        </CardHeader>
        <CardContent class="space-y-4">
          <p class="text-sm text-muted-foreground">
            <code>yerdd</code> is the background service that supervises PHP-FPM,
            serves your <code>.test</code> sites over HTTP/HTTPS, answers DNS, and
            runs databases. It runs unprivileged - this app is just a client.
          </p>
          <!-- Why a start attempt failed (only when the daemon is down). -->
          <DaemonDiagnosticsPanel v-if="startDiagnostics && !running" :diagnostics="startDiagnostics" />
          <div class="flex justify-end gap-2">
            <Button v-if="!running" :disabled="daemonStarting" @click="onStart">
              <Spinner v-if="daemonStarting" class="size-4" />
              <Play v-else class="size-4" /> {{ daemonStartLabel ?? "Start" }}
            </Button>
            <Button v-else variant="outline" :disabled="busy === 'daemon'" @click="onStop">
              <Spinner v-if="busy === 'daemon'" class="size-4" />
              <Square v-else class="size-4" /> Stop
            </Button>
            <Button
              variant="outline"
              :disabled="!running || busy === 'restart:daemon'"
              @click="openRestartDaemon"
            >
              <Spinner v-if="busy === 'restart:daemon'" class="size-4" />
              <RotateCw v-else class="size-4" /> Restart
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Health</CardTitle>
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
            <Button size="sm" :disabled="!hasActionable || fixing" @click="runFixes">
              <Spinner v-if="fixing" class="size-4" />
              <Wrench v-else class="size-4" /> Run safe fixes
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          <div v-if="diagLoading" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <div
            v-else-if="diagError"
            class="flex flex-col items-center gap-2 py-10 text-center"
          >
            <p class="text-sm font-medium">Health check unavailable</p>
            <p class="text-sm text-muted-foreground">
              Couldn't fetch diagnostics from the daemon.
            </p>
          </div>
          <div
            v-else-if="allClear"
            class="flex flex-col items-center gap-2 py-10 text-center"
          >
            <CheckCircle2 class="size-8 text-success" />
            <div>
              <p class="text-sm font-medium">No problems found</p>
              <p class="text-sm text-muted-foreground">
                Your Yerd environment looks healthy.
              </p>
            </div>
          </div>
          <ul v-else class="space-y-3">
            <li
              v-for="(d, i) in diagnoses"
              :key="i"
              class="flex items-start gap-3 rounded-md border p-3"
            >
              <Badge :variant="sevVariant[d.severity]" class="mt-0.5 shrink-0">{{ sevLabel[d.severity] }}</Badge>
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

      <!-- OS-level privileges (CA trust, .test resolver, privileged ports).
           Re-run the health checks after any elevation so the table above
           reflects the new state without a manual re-check. -->
      <EnvironmentCard @elevated="loadDiagnoses()" />
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
