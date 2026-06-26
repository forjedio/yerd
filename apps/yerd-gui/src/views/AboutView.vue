<script setup lang="ts">
import { getVersion } from "@tauri-apps/api/app";
import { FileText, Stethoscope } from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref } from "vue";

import logoUrl from "@/assets/logo.svg";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import { useToast } from "@/composables/useToast";
import {
  getDiagnostics,
  getGuiLogs,
  IpcError,
  openInBrowser,
  protocolVersion,
  status,
} from "@/ipc/client";
import type { GuiLogs } from "@/ipc/types";

const toast = useToast();

const appVersion = ref("");
const protocol = ref<number | null>(null);
const daemonVersion = ref("");

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

async function fetchLogs(): Promise<void> {
  try {
    logs.value = await getGuiLogs();
  } catch (e) {
    toast.error("Couldn't read logs", (e as IpcError).message);
  }
}

function openLogs(): void {
  logsOpen.value = true;
  void fetchLogs();
  // Light polling while open so a live daemon-start trail streams in.
  stopLogPolling();
  logsTimer = setInterval(() => void fetchLogs(), 2000);
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
          <div class="flex flex-wrap gap-2">
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
            <Button variant="outline" size="sm" @click="fetchLogs">Refresh</Button>
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
