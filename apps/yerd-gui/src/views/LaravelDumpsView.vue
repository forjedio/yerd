<script setup lang="ts">
import { ExternalLink } from "lucide-vue-next";
import { computed, onUnmounted } from "vue";

import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Switch from "@/components/ui/Switch.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { usePoll } from "@/composables/usePoll";
import { useToast } from "@/composables/useToast";
import {
  dumpsStatus,
  IpcError,
  setDumpFeature,
  setDumpsEnabled,
  showDumpsWindow,
} from "@/ipc/client";
import type { DumpsStatusResponse } from "@/ipc/types";

const toast = useToast();
const { data: status, refresh } = usePoll<DumpsStatusResponse>(dumpsStatus, 2500);

onUnmounted(registerViewActions({ refresh: () => void refresh() }));

const FEATURES: { key: string; label: string }[] = [
  { key: "dumps", label: "Dumps (dump / dd)" },
  { key: "queries", label: "Eloquent queries" },
  { key: "jobs", label: "Dispatched jobs" },
  { key: "views", label: "Blade views" },
  { key: "requests", label: "Incoming requests" },
  { key: "logs", label: "Log writes" },
  { key: "cache", label: "Cache events" },
  { key: "http", label: "Outgoing HTTP (curl / Guzzle)" },
];

const running = computed(() => status.value?.running ?? false);
const enabled = computed(() => status.value?.enabled ?? false);
const extensions = computed(() => status.value?.extensions ?? []);
const hasLegacy = computed(() => extensions.value.some((e) => e.legacy));

// The dump server *port* is now edited centrally on Settings ▸ Application Ports;
// this page shows it read-only via `status.port`.

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
    <PageHeader
      title="Dumps"
      subtitle="Intercept dump() calls and Laravel telemetry"
      docs="/guide/laravel-dumps"
    />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Dump interception</CardTitle>
          <CardDescription>
            Yerd can automatically intercept dump calls in your code and display them in a
            separate window - no codebase changes. Capture runs through a native PHP
            extension loaded into each site's PHP.
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-5">
          <!-- Server status -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Dump server status</p>
              <p class="text-xs text-muted-foreground">
                The loopback server that receives telemetry from PHP.
              </p>
            </div>
            <StatusPill
              :tone="running ? 'ok' : 'bad'"
              :label="running ? 'Running' : 'Stopped'"
              :pulse="running"
            />
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

          <!-- Port (read-only; edited centrally on Settings ▸ Application Ports) -->
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">Dump server port</p>
              <p class="text-xs text-muted-foreground">
                Change it in Settings ▸ Application Ports.
              </p>
            </div>
            <span class="font-mono text-sm">{{ status?.port ?? "-" }}</span>
          </div>

          <div class="flex justify-end border-t pt-4">
            <Button @click="openViewer">
              <ExternalLink class="size-4" /> Show Dumps
            </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Per-feature toggles -->
      <Card>
        <CardHeader>
          <CardTitle>Captured signals</CardTitle>
          <CardDescription>
            {{ enabled
              ? "Choose which telemetry to record."
              : "Enable interception above to record these signals." }}
          </CardDescription>
        </CardHeader>
        <CardContent
          class="space-y-3 transition-opacity"
          :class="{ 'opacity-50': !enabled }"
        >
          <div
            v-for="f in FEATURES"
            :key="f.key"
            class="flex items-center justify-between gap-4"
          >
            <p class="text-sm">{{ f.label }}</p>
            <Switch
              :model-value="featureOn(f.key)"
              :disabled="!enabled"
              :aria-label="f.label"
              @update:model-value="(v: boolean) => toggleFeature(f.key, v)"
            />
          </div>
        </CardContent>
      </Card>

      <!-- Legacy PHP warning -->
      <div
        v-if="hasLegacy"
        data-testid="dumps-legacy-banner"
        class="rounded-md border border-warning/40 bg-warning/10 px-4 py-3 text-sm"
      >
        One or more installed PHP versions are legacy (&lt; 8.2). Dumps are never captured for
        legacy versions - the <code>yerd-dump</code> extension isn't built for out-of-support PHP.
      </div>

      <!-- Extension presence -->
      <Card>
        <CardHeader>
          <CardTitle>PHP extension</CardTitle>
          <CardDescription>
            Telemetry is captured by the <code>yerd-dump</code> extension, installed per PHP
            version. Versions without it simply produce no dumps. Legacy PHP versions (&lt; 8.2)
            never capture dumps - the extension isn't built for them.
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
                <td class="py-2 font-mono">PHP {{ ext.version }}</td>
                <td class="py-2 text-right">
                  <Badge v-if="ext.legacy" variant="warning">Unsupported (no dumps)</Badge>
                  <Badge v-else :variant="ext.present ? 'success' : 'warning'">
                    {{ ext.present ? "Installed" : "Not installed" }}
                  </Badge>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
