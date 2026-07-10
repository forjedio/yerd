<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import {
  Copy,
  Database,
  Download,
  FileCode2,
  FileText,
  MoreHorizontal,
  Pencil,
  Play,
  Plus,
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
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  availableServices,
  backupDatabase,
  changeServiceVersion,
  createDatabase,
  dropDatabase,
  installService,
  IpcError,
  listDatabases,
  listServices,
  pickOpenFile,
  pickSaveFile,
  restartService,
  restoreDatabase,
  serviceLogs,
  setServicePort,
  startService,
  stopService,
  uninstallService,
} from "@/ipc/client";
import type { DatabaseSummary, ServiceStatus } from "@/ipc/types";
import { poolStateLabel, poolStateTone } from "@/lib/utils";

const toast = useToast();
const { refresh } = useDaemon();

// Cached SWR resource: a revisit renders the last service list instantly and
// revalidates underneath, so the table no longer flashes a spinner each time.
const { data, loading, error, refresh: load } = useResource("services", listServices);
const services = computed(() => data.value ?? []);
const busy = ref<string | null>(null); // a key naming the in-flight op (e.g. "start:redis")

// No AsyncState here, so surface a load failure as a toast - but only on a cold
// load (no cached data), so a transient background revalidation stays silent.
watch(error, (e) => {
  if (e && !data.value) toast.error("Couldn't load services", e.message);
});

function canStart(s: ServiceStatus): boolean {
  return s.installed_versions.length > 0 && s.state !== "running";
}
function canStop(s: ServiceStatus): boolean {
  return s.state === "running" || s.state === "failed";
}
function isInstalled(s: ServiceStatus): boolean {
  return s.installed_versions.length > 0;
}
/** The version to show: the active/selected one, falling back to what's on disk. */
function versionLabel(s: ServiceStatus): string {
  return s.selected_version ?? s.installed_versions[s.installed_versions.length - 1] ?? "-";
}

async function doStart(s: ServiceStatus): Promise<void> {
  busy.value = `start:${s.service}`;
  try {
    await startService(s.service);
    toast.success(`Started ${s.display_name}`);
    await Promise.all([load({ force: true }), refresh()]);
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
    await Promise.all([load({ force: true }), refresh()]);
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
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    toast.error(`Couldn't restart ${s.display_name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── version modal (shared by "Install" and "Change version") ──
// A service holds one installed version; both flows pick from the versions you
// don't currently have, so the option list is identical - only the action,
// titles, and empty-state copy differ by mode.
const installOpen = ref(false);
const installLoading = ref(false);
const installMode = ref<"install" | "change">("install");
const installTarget = ref<ServiceStatus | null>(null);
const installOptions = ref<{ value: string; label: string }[]>([]);
const selectedVersion = ref<string>("");

async function openVersionModal(s: ServiceStatus, mode: "install" | "change"): Promise<void> {
  installMode.value = mode;
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
    // Pre-select the LATEST (the daemon returns versions ascending, so the last
    // entry is newest) so the Select (no placeholder) is always valid.
    const opts = installOptions.value;
    selectedVersion.value = opts[opts.length - 1]?.value ?? "";
  } catch (e) {
    toast.error("Couldn't load versions", (e as IpcError).message);
  } finally {
    installLoading.value = false;
  }
}
const openInstall = (s: ServiceStatus) => openVersionModal(s, "install");
const openChange = (s: ServiceStatus) => openVersionModal(s, "change");

async function confirmInstall(close: () => void): Promise<void> {
  const s = installTarget.value;
  const v = selectedVersion.value;
  if (!s || !v) return;
  const mode = installMode.value;
  busy.value = `${mode}:${s.service}`;
  close();
  try {
    if (mode === "change") {
      await changeServiceVersion(s.service, v);
      toast.success(`Switched ${s.display_name} to v${v}`, "Restarted on the new version.");
    } else {
      await installService(s.service, v);
      toast.success(`Installed ${s.display_name} v${v}`, "Started and enabled on boot.");
    }
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    const verb = mode === "change" ? "Change" : "Install";
    toast.error(`${verb} of ${s.display_name} failed`, (e as IpcError).message);
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
    await load({ force: true });
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
  logsTimer.value = globalThis.setInterval(() => void fetchLogs(), 2000);
}

function stopLogPolling(): void {
  if (logsTimer.value !== null) {
    globalThis.clearInterval(logsTimer.value);
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

/**
 * Uninstall a service version. A retained-data notice comes back as a typed
 * error with a message (not a hard failure), so the catch surfaces that message
 * and still reloads the list.
 */
async function confirmUninstall(close: () => void): Promise<void> {
  const s = uninstallTarget.value;
  const v = uninstallVersion.value;
  if (!s || !v) return;
  busy.value = `uninstall:${s.service}`;
  close();
  try {
    await uninstallService(s.service, v, uninstallPurge.value);
    toast.success(`Uninstalled ${s.display_name} v${v}`);
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    toast.error(`Uninstall of ${s.display_name}`, (e as IpcError).message);
    await load({ force: true });
  } finally {
    busy.value = null;
  }
}

// ── manage databases modal ──
const dbOpen = ref(false);
const dbTarget = ref<ServiceStatus | null>(null);
const dbList = ref<DatabaseSummary[]>([]);
const dbLoading = ref(false);
const dbError = ref<string | null>(null);
const newDbName = ref("");
const dbActionBusy = ref(false);
const confirmDrop = ref<string | null>(null);
// A restore awaiting confirmation: the chosen file is picked first, then confirmed.
const confirmRestore = ref<{ name: string; path: string } | null>(null);

/** Mirror of the daemon's `validate_db_name` for instant feedback (the daemon
 *  re-validates authoritatively). */
function dbNameValid(name: string): boolean {
  return /^[A-Za-z_]\w{0,62}$/.test(name);
}

async function fetchDbs(): Promise<void> {
  const s = dbTarget.value;
  if (!s) return;
  dbLoading.value = true;
  dbError.value = null;
  try {
    dbList.value = await listDatabases(s.service);
  } catch (e) {
    dbError.value = (e as IpcError).message;
    dbList.value = [];
  } finally {
    dbLoading.value = false;
  }
}

async function openManageDb(s: ServiceStatus): Promise<void> {
  dbTarget.value = s;
  dbOpen.value = true;
  dbList.value = [];
  newDbName.value = "";
  confirmDrop.value = null;
  confirmRestore.value = null;
  await fetchDbs();
}

async function doCreateDb(): Promise<void> {
  const s = dbTarget.value;
  const name = newDbName.value.trim();
  if (!s || !dbNameValid(name)) return;
  dbActionBusy.value = true;
  try {
    await createDatabase(s.service, name);
    toast.success(`Created database ${name}`);
    newDbName.value = "";
    await fetchDbs();
  } catch (e) {
    toast.error("Couldn't create database", (e as IpcError).message);
  } finally {
    dbActionBusy.value = false;
  }
}

async function doDropDb(name: string): Promise<void> {
  const s = dbTarget.value;
  if (!s) return;
  dbActionBusy.value = true;
  confirmDrop.value = null;
  try {
    await dropDatabase(s.service, name);
    toast.success(`Dropped database ${name}`);
    await fetchDbs();
  } catch (e) {
    toast.error("Couldn't drop database", (e as IpcError).message);
  } finally {
    dbActionBusy.value = false;
  }
}

async function doBackupDb(name: string): Promise<void> {
  const s = dbTarget.value;
  if (!s) return;
  const path = await pickSaveFile(`${name}.sql`);
  if (!path) return; // user cancelled
  dbActionBusy.value = true;
  try {
    await backupDatabase(s.service, name, path);
    toast.success(`Backed up ${name}`, path);
  } catch (e) {
    toast.error("Couldn't back up database", (e as IpcError).message);
  } finally {
    dbActionBusy.value = false;
  }
}

/** Pick the file first, then ask for confirmation (restore overwrites data). */
async function startRestoreDb(name: string): Promise<void> {
  const path = await pickOpenFile();
  if (!path) return; // user cancelled
  confirmRestore.value = { name, path };
}

async function doRestoreDb(): Promise<void> {
  const s = dbTarget.value;
  const pending = confirmRestore.value;
  if (!s || !pending) return;
  dbActionBusy.value = true;
  confirmRestore.value = null;
  try {
    await restoreDatabase(s.service, pending.name, pending.path);
    toast.success(`Restored ${pending.name}`);
    await fetchDbs();
  } catch (e) {
    toast.error("Couldn't restore database", (e as IpcError).message);
  } finally {
    dbActionBusy.value = false;
  }
}

// ── Laravel configuration modal ──
// Shows the .env block to connect a Laravel app to this engine. For SQL engines
// the user can pick a database to pre-populate DB_DATABASE; for Redis it's the
// cache/session/queue block. Credentials mirror the daemon's: SQL engines bind
// localhost with no password (root for MySQL/MariaDB, postgres for Postgres).
const configOpen = ref(false);
const configTarget = ref<ServiceStatus | null>(null);
const configDbs = ref<DatabaseSummary[]>([]);
const configDbName = ref<string>("");
const configDbLoading = ref(false);
// Bumped on each open so a slow listDatabases can't overwrite a newer modal.
const configReqSeq = ref(0);

const configDbOptions = computed(() =>
  configDbs.value.map((d) => ({ value: d.name, label: d.name })),
);

async function openConfig(s: ServiceStatus): Promise<void> {
  const reqSeq = ++configReqSeq.value;
  configTarget.value = s;
  configDbName.value = "";
  configDbs.value = [];
  configOpen.value = true;
  // Only SQL engines have databases to choose, and only a running one can list them.
  if (s.supports_databases && s.state === "running") {
    configDbLoading.value = true;
    try {
      const dbs = await listDatabases(s.service);
      if (reqSeq !== configReqSeq.value) return; // a newer open superseded us
      configDbs.value = dbs;
      configDbName.value = dbs[0]?.name ?? "";
    } catch {
      if (reqSeq !== configReqSeq.value) return;
      configDbs.value = [];
    } finally {
      if (reqSeq === configReqSeq.value) configDbLoading.value = false;
    }
  }
}

function dbEnv(connection: string, port: number, database: string, user: string): string {
  return [
    `DB_CONNECTION=${connection}`,
    "DB_HOST=127.0.0.1",
    `DB_PORT=${port}`,
    `DB_DATABASE=${database}`,
    `DB_USERNAME=${user}`,
    "DB_PASSWORD=",
  ].join("\n");
}

const configSnippet = computed(() => {
  const s = configTarget.value;
  if (!s) return "";
  const db = configDbName.value.trim() || "your_database";
  switch (s.service) {
    case "redis":
      return [
        "REDIS_CLIENT=phpredis",
        "REDIS_HOST=127.0.0.1",
        "REDIS_PASSWORD=null",
        `REDIS_PORT=${s.port}`,
        "",
        "CACHE_STORE=redis",
        "SESSION_DRIVER=redis",
        "QUEUE_CONNECTION=redis",
      ].join("\n");
    case "mysql":
      return dbEnv("mysql", s.port, db, "root");
    case "mariadb":
      return dbEnv("mariadb", s.port, db, "root");
    case "postgres":
      return dbEnv("pgsql", s.port, db, "postgres");
    default:
      return "";
  }
});

async function copyConfig(): Promise<void> {
  try {
    await navigator.clipboard.writeText(configSnippet.value);
    toast.success("Copied to clipboard", "Paste these into your Laravel .env file.");
  } catch {
    toast.error("Couldn't copy");
  }
}

onUnmounted(stopLogPolling);
onUnmounted(registerViewActions({ refresh: () => void load() }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Services"
      subtitle="Databases and caches Yerd supervises"
      docs="/guide/services"
    />

    <div class="flex-1 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Local services</CardTitle>
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
                  <StatusPill
                    v-if="isInstalled(s)"
                    :tone="poolStateTone(s.state)"
                    :label="poolStateLabel(s.state)"
                  />
                  <span v-else class="text-xs italic text-muted-foreground">not installed</span>
                </td>
                <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">
                  {{ isInstalled(s) ? s.port : "-" }}
                </td>
                <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">
                  {{ versionLabel(s) }}
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
                        <DropdownMenuItem @select="openConfig(s)">
                          <FileCode2 class="size-4" /> Configuration
                        </DropdownMenuItem>
                        <DropdownMenuItem @select="openPort(s)">
                          <Pencil class="size-4" /> Edit port
                        </DropdownMenuItem>
                        <DropdownMenuItem @select="openLogs(s)">
                          <FileText class="size-4" /> View logs
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          v-if="s.supports_databases"
                          :disabled="s.state !== 'running'"
                          @select="openManageDb(s)"
                        >
                          <Database class="size-4" /> Manage databases
                        </DropdownMenuItem>
                        <DropdownMenuItem @select="openChange(s)">
                          <Download class="size-4" /> Change version
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

    <!-- Install / Change version -->
    <Modal
      v-model:open="installOpen"
      :title="
        installMode === 'change'
          ? `Change ${installTarget?.display_name ?? 'service'} version`
          : `Install ${installTarget?.display_name ?? 'service'}`
      "
    >
      <div v-if="installLoading" class="flex justify-center py-6"><Spinner class="size-5" /></div>
      <template v-else-if="installOptions.length">
        <span class="text-sm font-medium">Version</span>
        <div class="mt-2">
          <Select
            class="w-full"
            :model-value="selectedVersion"
            :options="installOptions"
            aria-label="version"
            @update:model-value="(v: string) => (selectedVersion = v)"
          />
        </div>
        <p class="mt-2 text-xs text-muted-foreground">
          <template v-if="installMode === 'change'">
            Installs the selected version, restarts the service onto it, and removes
            the current version. Your stored data is kept.
          </template>
          <template v-else>
            Downloads a prebuilt build; this can take a few minutes with no progress
            bar (the daemon reports only on completion).
          </template>
        </p>
      </template>
      <p v-else-if="installMode === 'change'" class="py-2 text-sm text-muted-foreground">
        No other versions to switch to - the installed version is the only one offered
        for this platform, or the distribution couldn't be reached.
      </p>
      <p v-else class="py-2 text-sm text-muted-foreground">
        No installable versions to add - every offered version is already installed,
        or the distribution couldn't be reached.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!installOptions.length || !selectedVersion" @click="confirmInstall(close)">
          {{ installMode === "change" ? "Switch" : "Install" }}
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
      size="full"
      :title="`${logsTarget?.display_name ?? 'Service'} logs`"
      @update:open="(o: boolean) => { if (!o) stopLogPolling(); }"
    >
      <pre
        v-if="logsLines.length"
        class="h-full overflow-auto rounded-md bg-muted p-3 text-xs leading-relaxed"
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

    <!-- Manage databases -->
    <Modal
      v-model:open="dbOpen"
      size="lg"
      :title="`${dbTarget?.display_name ?? 'Service'} databases`"
    >
      <div class="space-y-4">
        <!-- Create -->
        <div class="flex items-end gap-2">
          <div class="flex-1">
            <span class="text-sm font-medium">New database</span>
            <Input
              v-model="newDbName"
              class="mt-1"
              placeholder="my_app"
              @keyup.enter="doCreateDb"
            />
          </div>
          <Button
            :disabled="!dbNameValid(newDbName.trim()) || dbActionBusy"
            @click="doCreateDb"
          >
            <Plus class="size-4" /> Create
          </Button>
        </div>
        <p
          v-if="newDbName.trim() && !dbNameValid(newDbName.trim())"
          class="text-xs text-destructive"
        >
          Use letters, digits, and underscores; start with a letter or underscore (max 63).
        </p>

        <!-- List -->
        <div v-if="dbLoading" class="flex justify-center py-6"><Spinner class="size-5" /></div>
        <p v-else-if="dbError" class="py-2 text-sm text-muted-foreground">{{ dbError }}</p>
        <p v-else-if="!dbList.length" class="py-2 text-sm text-muted-foreground">
          No databases yet.
        </p>
        <ul v-else class="divide-y rounded-md border">
          <li v-for="d in dbList" :key="d.name" class="flex items-center gap-2 px-3 py-2">
            <template v-if="confirmDrop === d.name">
              <span class="flex-1 text-sm">
                Delete <span class="font-mono">{{ d.name }}</span>? This cannot be undone.
              </span>
              <Button
                size="sm"
                variant="destructive"
                :disabled="dbActionBusy"
                @click="doDropDb(d.name)"
              >
                Confirm
              </Button>
              <Button size="sm" variant="ghost" @click="confirmDrop = null">Cancel</Button>
            </template>
            <template v-else-if="confirmRestore?.name === d.name">
              <span class="flex-1 text-sm">
                Restore into <span class="font-mono">{{ d.name }}</span>? Existing data will be
                overwritten.
              </span>
              <Button
                size="sm"
                variant="destructive"
                :disabled="dbActionBusy"
                @click="doRestoreDb()"
              >
                Confirm
              </Button>
              <Button size="sm" variant="ghost" @click="confirmRestore = null">Cancel</Button>
            </template>
            <template v-else>
              <span class="flex-1 font-mono text-sm">{{ d.name }}</span>
              <Button
                size="sm"
                variant="ghost"
                :disabled="dbActionBusy"
                @click="doBackupDb(d.name)"
              >
                Backup
              </Button>
              <Button
                size="sm"
                variant="ghost"
                :disabled="dbActionBusy"
                @click="startRestoreDb(d.name)"
              >
                Restore
              </Button>
              <Button
                size="sm"
                variant="ghost"
                class="text-destructive focus:text-destructive"
                :disabled="dbActionBusy"
                @click="confirmDrop = d.name"
              >
                <Trash2 class="size-4" /> Delete
              </Button>
            </template>
          </li>
        </ul>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Close</Button>
      </template>
    </Modal>

    <!-- Laravel configuration -->
    <Modal
      v-model:open="configOpen"
      :title="`${configTarget?.display_name ?? 'Service'} configuration`"
    >
      <div class="space-y-4">
        <p class="text-sm text-muted-foreground">
          Add these to your Laravel app's <code>.env</code> to connect it to
          {{ configTarget?.display_name }}.
        </p>

        <!-- Database picker (SQL engines only) -->
        <div v-if="configTarget?.supports_databases">
          <span class="text-sm font-medium">Database</span>
          <div v-if="configDbLoading" class="mt-2 flex py-1"><Spinner class="size-4" /></div>
          <Select
            v-else-if="configDbOptions.length"
            class="mt-2 w-full"
            :model-value="configDbName"
            :options="configDbOptions"
            aria-label="Database"
            @update:model-value="(v: string) => (configDbName = v)"
          />
          <p v-else class="mt-1 text-xs text-muted-foreground">
            {{
              configTarget?.state === "running"
                ? "No databases yet - create one under \"Manage databases\". Using a placeholder below."
                : "Start the service to list databases. Using a placeholder below."
            }}
          </p>
        </div>

        <!-- .env snippet -->
        <div class="relative">
          <pre class="overflow-x-auto rounded-md border bg-muted/50 p-3 font-mono text-xs leading-relaxed text-foreground">{{ configSnippet }}</pre>
        </div>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Close</Button>
        <Button @click="copyConfig"><Copy class="size-4" /> Copy</Button>
      </template>
    </Modal>
  </div>
</template>
