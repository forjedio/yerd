<script setup lang="ts">
import { ExternalLink } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Input from "@/components/ui/Input.vue";
import Switch from "@/components/ui/Switch.vue";
import { usePoll } from "@/composables/usePoll";
import { useToast } from "@/composables/useToast";
import {
  dumpsStatus,
  IpcError,
  setDumpFeature,
  setDumpsEnabled,
  setDumpsPort,
  showDumpsWindow,
} from "@/ipc/client";
import type { DumpsStatusResponse } from "@/ipc/types";

const toast = useToast();
const { data: status, refresh } = usePoll<DumpsStatusResponse>(dumpsStatus, 2500);

const FEATURES: { key: string; label: string }[] = [
  { key: "dumps", label: "Dumps (dump / dd)" },
  { key: "queries", label: "Eloquent queries" },
  { key: "jobs", label: "Dispatched jobs" },
  { key: "views", label: "Blade views" },
  { key: "requests", label: "HTTP requests" },
  { key: "logs", label: "Log writes" },
  { key: "cache", label: "Cache events" },
];

const running = computed(() => status.value?.running ?? false);
const enabled = computed(() => status.value?.enabled ?? false);
const extensions = computed(() => status.value?.extensions ?? []);

// Port draft, synced from the daemon until the user edits it.
const portDraft = ref("");
let portDirty = false;
watch(
  status,
  (s) => {
    if (s && !portDirty) portDraft.value = String(s.port);
  },
  { immediate: true },
);
function onPortInput(v: string): void {
  portDraft.value = v;
  portDirty = true;
}
const portChanged = computed(
  () => status.value !== null && portDraft.value !== String(status.value.port),
);

async function savePort(): Promise<void> {
  const port = Number.parseInt(portDraft.value, 10);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    toast.error("Invalid port", "Enter a number between 1 and 65535.");
    return;
  }
  try {
    await setDumpsPort(port);
    portDirty = false;
    await refresh();
    toast.success("Dump server port updated");
  } catch (e) {
    toast.error("Couldn't set port", (e as IpcError).message);
  }
}

async function toggleEnabled(next: boolean): Promise<void> {
  try {
    await setDumpsEnabled(next);
    await refresh();
  } catch (e) {
    toast.error("Couldn't toggle interception", (e as IpcError).message);
  }
}

function featureOn(key: string): boolean {
  return status.value?.features?.[key] ?? true;
}
async function toggleFeature(key: string, next: boolean): Promise<void> {
  try {
    await setDumpFeature(key, next);
    await refresh();
  } catch (e) {
    toast.error("Couldn't update feature", (e as IpcError).message);
  }
}

async function openViewer(): Promise<void> {
  try {
    await showDumpsWindow();
  } catch (e) {
    toast.error("Couldn't open the dumps window", (e as IpcError).message);
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Dumps" subtitle="Intercept dump() calls and Laravel telemetry" />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Dump interception</CardTitle>
          <CardDescription>
            Yerd can automatically intercept dump calls in your code and display them in a
            separate window — no codebase changes. Capture runs through a native PHP
            extension loaded into each site's PHP.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-5">
          <!-- Server status -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Dump Server Status</p>
              <p class="text-xs text-muted-foreground">
                The loopback server that receives telemetry from PHP.
              </p>
            </div>
            <span class="flex items-center gap-2 text-sm">
              <span
                class="size-2.5 rounded-full"
                :class="running ? 'bg-green-500' : 'bg-red-500'"
              />
              {{ running ? "Running" : "Stopped" }}
            </span>
          </div>

          <!-- Antenna toggle -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Enable interception</p>
              <p class="text-xs text-muted-foreground">
                The “antenna”. When on, captured dumps stream to the viewer.
              </p>
            </div>
            <Switch
              :model-value="enabled"
              aria-label="Enable dump interception"
              @update:model-value="toggleEnabled"
            />
          </div>

          <!-- Port -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Dump Server Port</p>
              <p class="text-xs text-muted-foreground">
                The port the dump server listens on (default 2304).
              </p>
            </div>
            <div class="flex items-center gap-2">
              <Input
                :model-value="portDraft"
                type="number"
                class="w-28"
                @update:model-value="onPortInput"
              />
              <Button size="sm" :disabled="!portChanged" @click="savePort">Save</Button>
            </div>
          </div>

          <div class="flex items-center gap-3 pt-1">
            <Button variant="secondary" @click="openViewer">Show Dumps</Button>
            <a
              href="https://herd.laravel.com/docs/macos/debugging/dumps"
              target="_blank"
              rel="noopener"
              class="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
            >
              Learn how dump interception works
              <ExternalLink class="size-3" />
            </a>
          </div>
        </CardContent>
      </Card>

      <!-- Per-feature toggles -->
      <Card>
        <CardHeader>
          <CardTitle>Captured signals</CardTitle>
          <CardDescription>Choose which telemetry to record.</CardDescription>
        </CardHeader>
        <CardContent class="space-y-3">
          <div
            v-for="f in FEATURES"
            :key="f.key"
            class="flex items-center justify-between gap-4"
          >
            <p class="text-sm">{{ f.label }}</p>
            <Switch
              :model-value="featureOn(f.key)"
              :aria-label="f.label"
              @update:model-value="(v: boolean) => toggleFeature(f.key, v)"
            />
          </div>
        </CardContent>
      </Card>

      <!-- Extension presence -->
      <Card>
        <CardHeader>
          <CardTitle>PHP extension</CardTitle>
          <CardDescription>
            Telemetry is captured by the <code>yerd-dump</code> extension, installed per PHP
            version. Versions without it simply produce no dumps.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <p v-if="extensions.length === 0" class="text-sm text-muted-foreground">
            No PHP versions installed.
          </p>
          <table v-else class="w-full text-sm">
            <tbody>
              <tr
                v-for="ext in extensions"
                :key="ext.version"
                class="border-b last:border-0"
              >
                <td class="py-2">PHP {{ ext.version }}</td>
                <td class="py-2 text-right">
                  <span
                    class="rounded px-2 py-0.5 text-xs"
                    :class="
                      ext.present
                        ? 'bg-green-500/15 text-green-600 dark:text-green-400'
                        : 'bg-amber-500/15 text-amber-600 dark:text-amber-400'
                    "
                  >
                    {{ ext.present ? "Installed" : "Not installed" }}
                  </span>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
