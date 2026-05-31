<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import {
  Database,
  Download,
  FileText,
  MoreHorizontal,
  Pencil,
  Play,
  RotateCw,
  Square,
  Trash2,
} from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
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
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  availableServices,
  installService,
  IpcError,
  listServices,
  restartService,
  serviceLogs,
  setServicePort,
  startService,
  stopService,
  uninstallService,
} from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { poolStateLabel, poolStateTone } from "@/lib/utils";

const toast = useToast();
const { refresh } = useDaemon();

const services = ref<ServiceStatus[]>([]);
const loading = ref(true);
const busy = ref<string | null>(null); // a key naming the in-flight op (e.g. "start:redis")

async function load(): Promise<void> {
  try {
    services.value = await listServices();
  } catch (e) {
    toast.error("Couldn't load services", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

function canStart(s: ServiceStatus): boolean {
  return s.installed_versions.length > 0 && s.state !== "running";
}
function canStop(s: ServiceStatus): boolean {
  return s.state === "running" || s.state === "failed";
}

async function doStart(s: ServiceStatus): Promise<void> {
  busy.value = `start:${s.service}`;
  try {
    await startService(s.service);
    toast.success(`Started ${s.display_name}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    toast.error(`Couldn't start ${s.display_name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doStop(s: ServiceStatus): Promise<void> {
  busy.value = `stop:${s.service}`;
  try {
    await stopService(s.service);
    toast.success(`Stopped ${s.display_name}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    toast.error(`Couldn't stop ${s.display_name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doRestart(s: ServiceStatus): Promise<void> {
  busy.value = `restart:${s.service}`;
  try {
    await restartService(s.service);
    toast.success(`Restarted ${s.display_name}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    toast.error(`Couldn't restart ${s.display_name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── install modal ──
const installOpen = ref(false);
const installLoading = ref(false);
const installTarget = ref<ServiceStatus | null>(null);
const installOptions = ref<{ value: string; label: string }[]>([]);
const selectedVersion = ref<string>("");

async function openInstall(s: ServiceStatus): Promise<void> {
  installTarget.value = s;
  installOpen.value = true;
  installLoading.value = true;
  installOptions.value = [];
  selectedVersion.value = "";
  try {
    const all = await availableServices();
    const entry = all.find((a) => a.service === s.service);
    const installedSet = new Set(entry?.installed ?? []);
    installOptions.value = (entry?.available ?? [])
      .filter((v) => !installedSet.has(v))
      .map((v) => ({ value: v, label: `v${v}` }));
    selectedVersion.value = installOptions.value[0]?.value ?? "";
  } catch (e) {
    toast.error("Couldn't load installable versions", (e as IpcError).message);
  } finally {
    installLoading.value = false;
  }
}

async function confirmInstall(close: () => void): Promise<void> {
  const s = installTarget.value;
  const v = selectedVersion.value;
  if (!s || !v) return;
  busy.value = `install:${s.service}`;
  close();
  try {
    await installService(s.service, v);
    toast.success(`Installed ${s.display_name} v${v}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    toast.error(`Install of ${s.display_name} failed`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── edit-port modal ──
const portOpen = ref(false);
const portTarget = ref<ServiceStatus | null>(null);
const portValue = ref<string>("");

function openPort(s: ServiceStatus): void {
  portTarget.value = s;
  portValue.value = String(s.port);
  portOpen.value = true;
}

async function confirmPort(close: () => void): Promise<void> {
  const s = portTarget.value;
  const port = Number(portValue.value);
  if (!s || !Number.isInteger(port) || port < 1 || port > 65535) {
    toast.error("Invalid port", "Enter a number between 1 and 65535.");
    return;
  }
  busy.value = `port:${s.service}`;
  close();
  try {
    await setServicePort(s.service, port);
    toast.success(`${s.display_name} port set to ${port}`, "Restart the service to apply.");
    await load();
  } catch (e) {
    toast.error(`Couldn't set ${s.display_name} port`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── logs modal (polled only while open) ──
const logsOpen = ref(false);
const logsTarget = ref<ServiceStatus | null>(null);
const logsLines = ref<string[]>([]);
const logsTimer = ref<number | null>(null);

async function fetchLogs(): Promise<void> {
  const s = logsTarget.value;
  if (!s) return;
  try {
    logsLines.value = await serviceLogs(s.service, 200);
  } catch (e) {
    toast.error("Couldn't read logs", (e as IpcError).message);
  }
}

async function openLogs(s: ServiceStatus): Promise<void> {
  logsTarget.value = s;
  logsLines.value = [];
  logsOpen.value = true;
  await fetchLogs();
  // Poll while the modal is open; cleared on close / unmount.
  logsTimer.value = window.setInterval(() => void fetchLogs(), 2000);
}

function stopLogPolling(): void {
  if (logsTimer.value !== null) {
    window.clearInterval(logsTimer.value);
    logsTimer.value = null;
  }
}

// ── uninstall modal ──
const uninstallOpen = ref(false);
const uninstallTarget = ref<ServiceStatus | null>(null);
const uninstallVersion = ref<string>("");
const uninstallPurge = ref(false);

function openUninstall(s: ServiceStatus): void {
  uninstallTarget.value = s;
  uninstallVersion.value = s.installed_versions[s.installed_versions.length - 1] ?? "";
  uninstallPurge.value = false;
  uninstallOpen.value = true;
}

async function confirmUninstall(close: () => void): Promise<void> {
  const s = uninstallTarget.value;
  const v = uninstallVersion.value;
  if (!s || !v) return;
  busy.value = `uninstall:${s.service}`;
  close();
  try {
    await uninstallService(s.service, v, uninstallPurge.value);
    toast.success(`Uninstalled ${s.display_name} v${v}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    // A retained-data notice comes back as an error code with a message; show it.
    toast.error(`Uninstall of ${s.display_name}`, (e as IpcError).message);
    await load();
  } finally {
    busy.value = null;
  }
}

onMounted(load);
onUnmounted(stopLogPolling);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Services" subtitle="Databases and caches Yerd supervises" />

    <div class="flex-1 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center gap-2">
            <Database class="size-4" /> Local services
          </CardTitle>
          <CardDescription>
            Each engine binds to localhost only with no password. Install a version,
            then start it; changes to the port apply on the next restart.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-12"><Spinner class="size-6" /></div>

          <table v-else class="w-full text-sm">
            <thead>
              <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                <th class="py-2 pr-4 font-medium">Service</th>
                <th class="py-2 pr-4 font-medium">State</th>
                <th class="py-2 pr-4 font-medium">Port</th>
                <th class="py-2 pr-4 font-medium">Version</th>
                <th class="py-2 pr-4 font-medium">Installed</th>
                <th class="py-2 pl-4 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="s in services" :key="s.service" class="border-b last:border-0">
                <td class="py-3 pr-4">
                  <div class="flex items-center gap-2">
                    <span class="font-medium">{{ s.display_name }}</span>
                    <Badge v-if="s.supports_databases" variant="secondary">SQL</Badge>
                  </div>
                </td>
                <td class="py-3 pr-4">
                  <StatusPill :tone="poolStateTone(s.state)" :label="poolStateLabel(s.state)" />
                </td>
                <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">{{ s.port }}</td>
                <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">
                  {{ s.selected_version ?? "—" }}
                </td>
                <td class="py-3 pr-4 text-xs text-muted-foreground">
                  <span v-if="s.installed_versions.length">{{ s.installed_versions.join(", ") }}</span>
                  <span v-else class="italic">not installed</span>
                </td>
                <td class="py-3 pl-4">
                  <div class="flex items-center justify-end gap-2">
                    <Spinner v-if="busy?.endsWith(`:${s.service}`)" class="size-4" />
                    <Button
                      v-if="!s.installed_versions.length"
                      size="sm"
                      :disabled="busy === `install:${s.service}`"
                      @click="openInstall(s)"
                    >
                      <Download class="size-4" /> Install
                    </Button>
                    <DropdownMenu v-else>
                      <DropdownMenuTrigger as-child>
                        <Button variant="ghost" size="icon" :aria-label="`Actions for ${s.display_name}`">
                          <MoreHorizontal class="size-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem :disabled="!canStart(s)" @select="doStart(s)">
                          <Play class="size-4" /> Start
                        </DropdownMenuItem>
                        <DropdownMenuItem :disabled="!canStop(s)" @select="doStop(s)">
                          <Square class="size-4" /> Stop
                        </DropdownMenuItem>
                        <DropdownMenuItem :disabled="!canStop(s)" @select="doRestart(s)">
                          <RotateCw class="size-4" /> Restart
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem @select="openPort(s)">
                          <Pencil class="size-4" /> Edit port
                        </DropdownMenuItem>
                        <DropdownMenuItem @select="openLogs(s)">
                          <FileText class="size-4" /> View logs
                        </DropdownMenuItem>
                        <DropdownMenuItem @select="openInstall(s)">
                          <Download class="size-4" /> Install version
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                          @select="openUninstall(s)"
                        >
                          <Trash2 class="size-4" /> Uninstall
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>
    </div>

    <!-- Install -->
    <Modal v-model:open="installOpen" :title="`Install ${installTarget?.display_name ?? 'service'}`">
      <div v-if="installLoading" class="flex justify-center py-6"><Spinner class="size-5" /></div>
      <template v-else-if="installOptions.length">
        <span class="text-sm font-medium">Version</span>
        <div class="mt-2">
          <Select
            class="w-full"
            :model-value="selectedVersion"
            :options="installOptions"
            aria-label="version to install"
            @update:model-value="(v: string) => (selectedVersion = v)"
          />
        </div>
        <p class="mt-2 text-xs text-muted-foreground">
          Downloads a prebuilt build; this can take a few minutes with no progress
          bar (the daemon reports only on completion).
        </p>
      </template>
      <p v-else class="py-2 text-sm text-muted-foreground">
        No installable versions to add — every offered version is already installed,
        or the distribution couldn't be reached.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!installOptions.length || !selectedVersion" @click="confirmInstall(close)">
          Install
        </Button>
      </template>
    </Modal>

    <!-- Edit port -->
    <Modal v-model:open="portOpen" :title="`Edit ${portTarget?.display_name ?? 'service'} port`">
      <span class="text-sm font-medium">Port</span>
      <Input v-model="portValue" type="number" min="1" max="65535" class="mt-2" />
      <p class="mt-2 text-xs text-muted-foreground">
        The service binds 127.0.0.1 on this port. Restart the service to apply.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmPort(close)">Save</Button>
      </template>
    </Modal>

    <!-- Logs -->
    <Modal
      v-model:open="logsOpen"
      :title="`${logsTarget?.display_name ?? 'Service'} logs`"
      @update:open="(o: boolean) => { if (!o) stopLogPolling(); }"
    >
      <pre
        v-if="logsLines.length"
        class="max-h-96 overflow-auto rounded-md bg-muted p-3 text-xs leading-relaxed"
      >{{ logsLines.join("\n") }}</pre>
      <p v-else class="py-2 text-sm text-muted-foreground">No log output yet.</p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="(stopLogPolling(), close())">Close</Button>
      </template>
    </Modal>

    <!-- Uninstall -->
    <Modal v-model:open="uninstallOpen" :title="`Uninstall ${uninstallTarget?.display_name ?? 'service'}`">
      <p class="text-sm text-muted-foreground">
        Remove
        <strong class="font-mono text-foreground">{{ uninstallTarget?.display_name }} v{{ uninstallVersion }}</strong>?
        This stops the service. Your stored data is kept unless you tick purge.
      </p>
      <label class="mt-3 flex items-center gap-2 text-sm">
        <input v-model="uninstallPurge" type="checkbox" class="size-4" />
        Also delete stored data (cannot be undone)
      </label>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmUninstall(close)">Uninstall</Button>
      </template>
    </Modal>
  </div>
</template>
