<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { Database, Plus, Trash2 } from "lucide-vue-next";

import AddServiceDialog, { type AddServiceRequest } from "@/components/services/AddServiceDialog.vue";
import ServiceInspector, { type DatabaseListState } from "@/components/services/ServiceInspector.vue";
import ServiceLogsDialog from "@/components/services/ServiceLogsDialog.vue";
import ServicesList from "@/components/services/ServicesList.vue";
import PageHeader from "@/components/PageHeader.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  addService,
  availableServices,
  changeServiceVersion,
  IpcError,
  listDatabases,
  listServices,
  openInBrowser,
  removeService,
  restartService,
  setServiceAutostart,
  setServicePort,
  sitesAndParked,
  startService,
  stopService,
  uninstallService,
} from "@/ipc/client";
import type { ServiceStatus, SiteEntry } from "@/ipc/types";
import {
  changeVersionNote,
  currentVersion,
  isInstalled,
  isPerSite,
  purgeCopy,
  serviceSections,
  siteDomain,
  uninstallVersion,
  versionChoices,
  type VersionChoice,
} from "@/lib/services";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { siteUrl } from "@/lib/siteUrl";
import { toastOnError } from "@/lib/toastActions";

const toast = useToast();
const { refresh, report } = useDaemon();
const wide = useWideLayout();

const { data, loading, error: resourceError, refresh: load } = useResource("services", listServices);
usePoll(() => load(), 5000);

const { data: sitesData } = useResource("sites", sitesAndParked);

const services = computed<ServiceStatus[]>(() => data.value ?? []);
const sites = computed<SiteEntry[]>(() => sitesData.value?.sites ?? []);
const tld = computed(() => report.value?.tld ?? "test");
const sections = computed(() => serviceSections(services.value));
const items = computed(() => sections.value.flatMap((s) => s.items));

/** Only a cold-load failure replaces the screen; a failed revalidation over
 *  cached data keeps the list and inspector. */
const error = computed(() => (data.value ? null : (resourceError.value?.message ?? null)));

/** Keys naming every in-flight service action, e.g. `start:redis` or
 *  `add:meilisearch`. Each operation adds and removes only its own key, so one
 *  finishing never unlocks another that is still running. */
const busyKeys = ref<string[]>([]);

/** Marks `key` busy until the returned callback runs. */
function beginBusy(key: string): () => void {
  busyKeys.value = [...busyKeys.value, key];
  return () => {
    const i = busyKeys.value.indexOf(key);
    if (i >= 0) busyKeys.value = busyKeys.value.filter((_, j) => j !== i);
  };
}

/** The in-flight action on the selected service, for the inspector. */
const selectedBusy = computed(
  () => busyKeys.value.find((k) => !!selectedId.value && k.endsWith(`:${selectedId.value}`)) ?? null,
);

/** An add is in flight. The service has no row until it is installed, so the
 *  Add dialog carries the progress: it stays open and locked until the call
 *  settles. */
const adding = computed(() => busyKeys.value.some((k) => k.startsWith("add:")));

// ── database lists ──
/**
 * The database names of each running SQL engine, shared by the list row's
 * count, the table's DETAILS, the Uninstall copy, the Databases card and the
 * Connect picker. Every `list_databases` spawns the engine's client, so lists
 * load when an engine starts running (once until it stops again), on a forced
 * reload (⌘R, a card mutation, picking another engine) and never on the 5s
 * poll. A per-id sequence number drops stale replies.
 */
const dbLists = ref<Record<string, DatabaseListState>>({});
const dbSeq = new Map<string, number>();
const listedSinceStart = new Set<string>();

const runningSql = computed(() =>
  services.value
    .filter((s) => s.supports_databases && !isPerSite(s) && s.state === "running")
    .map((s) => s.service)
    .sort(),
);
const runningSqlKey = computed(() => runningSql.value.join(","));

const dbCounts = computed<Record<string, number>>(() => {
  const counts: Record<string, number> = {};
  for (const [id, list] of Object.entries(dbLists.value)) {
    if (list.names) counts[id] = list.names.length;
  }
  return counts;
});

function isRunningSql(id: string | null): id is string {
  return !!id && runningSql.value.includes(id);
}

function setList(id: string, next: DatabaseListState): void {
  dbLists.value = { ...dbLists.value, [id]: next };
}

async function loadDatabases(id: string): Promise<void> {
  const seq = (dbSeq.get(id) ?? 0) + 1;
  dbSeq.set(id, seq);
  listedSinceStart.add(id);
  setList(id, { names: dbLists.value[id]?.names ?? null, loading: true, error: null });
  try {
    const dbs = await listDatabases(id);
    if (dbSeq.get(id) !== seq) return;
    setList(id, { names: dbs.map((d) => d.name), loading: false, error: null });
  } catch (e) {
    if (dbSeq.get(id) !== seq) return;
    listedSinceStart.delete(id);
    setList(id, { names: null, loading: false, error: (e as IpcError).message });
  }
}

/** List an engine's databases unless that already happened since it last
 *  started running (or is in flight). */
function ensureDatabases(id: string): void {
  if (!listedSinceStart.has(id)) void loadDatabases(id);
}

watch(
  runningSqlKey,
  () => {
    const running = new Set(runningSql.value);
    const kept: Record<string, DatabaseListState> = {};
    for (const [id, list] of Object.entries(dbLists.value)) {
      if (running.has(id)) kept[id] = list;
    }
    for (const id of [...listedSinceStart]) {
      if (running.has(id)) continue;
      listedSinceStart.delete(id);
      dbSeq.set(id, (dbSeq.get(id) ?? 0) + 1);
    }
    dbLists.value = kept;
    for (const id of running) ensureDatabases(id);
  },
  { immediate: true },
);

function reloadAll(): void {
  void load({ force: true });
  for (const id of runningSql.value) void loadDatabases(id);
}

const emptyList: DatabaseListState = { names: null, loading: false, error: null };

// ── selection ──
const selectedId = ref<string | null>(null);
const drawerOpen = ref(false);
const selected = computed(() => items.value.find((s) => s.service === selectedId.value) ?? null);
const selectedDatabases = computed(() =>
  selectedId.value ? (dbLists.value[selectedId.value] ?? emptyList) : emptyList,
);

watch(
  [items, wide],
  ([list, isWide]) => {
    if (!isWide) return;
    if (!selectedId.value || !list.some((s) => s.service === selectedId.value)) {
      selectedId.value = list[0]?.service ?? null;
    }
  },
  { immediate: true },
);

watch(selectedId, (id) => {
  if (isRunningSql(id)) ensureDatabases(id);
});

watch(selected, (s) => {
  if (!s) drawerOpen.value = false;
});

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

/** A row click. Picking a different engine re-lists its databases; in the
 *  compact table every click opens the drawer and re-lists, so reopening the
 *  same row shows fresh names. Clicking a row whose last listing failed (an
 *  engine that reported running before it took connections) retries it. */
function select(id: string): void {
  const changed = id !== selectedId.value;
  selectedId.value = id;
  if (!wide.value) drawerOpen.value = true;
  const failed = !!dbLists.value[id]?.error;
  if (isRunningSql(id) && (changed || failed || !wide.value)) void loadDatabases(id);
}

// ── service actions ──
async function runAction(
  s: ServiceStatus,
  action: string,
  call: () => Promise<void>,
  done: string,
  failed: string,
): Promise<void> {
  const end = beginBusy(`${action}:${s.service}`);
  try {
    await call();
    toast.success(done);
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    toast.error(failed, (e as IpcError).message);
  } finally {
    end();
  }
}

function doStart(s: ServiceStatus): Promise<void> {
  return runAction(s, "start", () => startService(s.service), `Started ${s.display_name}`, `Couldn't start ${s.display_name}`);
}

function doStop(s: ServiceStatus): Promise<void> {
  return runAction(s, "stop", () => stopService(s.service), `Stopped ${s.display_name}`, `Couldn't stop ${s.display_name}`);
}

function doRestart(s: ServiceStatus): Promise<void> {
  return runAction(
    s,
    "restart",
    () => restartService(s.service),
    `Restarted ${s.display_name}`,
    `Couldn't restart ${s.display_name}`,
  );
}

function doRemove(s: ServiceStatus): Promise<void> {
  return runAction(
    s,
    "remove",
    () => removeService(s.service, false),
    `Removed ${s.display_name}`,
    `Couldn't remove ${s.display_name}`,
  );
}

function doToggleAutostart(s: ServiceStatus, enabled: boolean): Promise<void> {
  return runAction(
    s,
    "autostart",
    () => setServiceAutostart(s.service, enabled),
    enabled ? `${s.display_name} starts with Yerd` : `${s.display_name} won't start with Yerd`,
    `Couldn't update ${s.display_name}`,
  );
}

async function openSite(site: SiteEntry): Promise<void> {
  await toastOnError("Couldn't open site", () => openInBrowser(siteUrl(site, report.value)));
}

// ── add ──
const addOpen = ref(false);

function openAdd(): void {
  addOpen.value = true;
}

/** Ids of the configured instances. A single-instance engine is listed even
 *  before it is installed, so "saved" means an instance turned configured, not
 *  that a new id appeared. */
function configuredIds(): Set<string> {
  return new Set(services.value.filter(isInstalled).map((s) => s.service));
}

/** The daemon saves an instance before starting it, so a failed start still
 *  leaves it added: that closes the dialog with a start-failure toast, and only
 *  a failure that saved nothing keeps the dialog open for a retry. */
async function confirmAdd(request: AddServiceRequest, name: string): Promise<void> {
  const end = beginBusy(`add:${request.type_id}`);
  const before = configuredIds();
  try {
    await addService(request);
    toast.success(`Added ${name}`);
    addOpen.value = false;
  } catch (e) {
    await load({ force: true });
    const saved = [...configuredIds()].some((id) => !before.has(id));
    if (saved) {
      toast.error(`${name} added, but couldn't start`, (e as IpcError).message);
      addOpen.value = false;
    } else {
      toast.error(`Couldn't add ${name}`, (e as IpcError).message);
    }
  } finally {
    end();
  }
  await Promise.all([load({ force: true }), refresh()]);
}

// ── change version ──
const versionOpen = ref(false);
const versionLoading = ref(false);
const versionTarget = ref<ServiceStatus | null>(null);
const versionOptions = ref<VersionChoice[]>([]);
const selectedVersion = ref("");

/** With no newer version there is no preselection, so the picker leads with a
 *  placeholder rather than showing an older version it hasn't selected. */
const versionSelectOptions = computed(() =>
  versionOptions.value.every((o) => o.downgrade)
    ? [{ value: "", label: "Choose a version" }, ...versionOptions.value]
    : versionOptions.value,
);

const versionNote = computed(() => {
  const s = versionTarget.value;
  if (!s) return { text: "", blocked: true };
  return changeVersionNote(s.type_id ?? s.service, s.display_name, currentVersion(s), selectedVersion.value);
});

async function openChange(s: ServiceStatus): Promise<void> {
  versionTarget.value = s;
  versionOpen.value = true;
  versionLoading.value = true;
  versionOptions.value = [];
  selectedVersion.value = "";
  try {
    const all = await availableServices();
    const entry = all.find((a) => a.service === s.service);
    const choices = versionChoices(entry?.available ?? [], entry?.installed ?? [], currentVersion(s));
    versionOptions.value = choices.options;
    selectedVersion.value = choices.preselect;
  } catch (e) {
    toast.error("Couldn't load versions", (e as IpcError).message);
  } finally {
    versionLoading.value = false;
  }
}

async function confirmChange(close: () => void): Promise<void> {
  const s = versionTarget.value;
  const v = selectedVersion.value;
  if (!s || !v || versionNote.value.blocked) return;
  const end = beginBusy(`change:${s.service}`);
  close();
  try {
    await changeServiceVersion(s.service, v);
    toast.success(`Switched ${s.display_name} to v${v}`, "Restarted on the new version.");
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    toast.error(`Change of ${s.display_name} failed`, (e as IpcError).message);
  } finally {
    end();
  }
}

// ── edit port ──
const portOpen = ref(false);
const portTarget = ref<ServiceStatus | null>(null);
const portValue = ref("");

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
  const end = beginBusy(`port:${s.service}`);
  close();
  try {
    await setServicePort(s.service, port);
    toast.success(`${s.display_name} port set to ${port}`, "Restart the service to apply.");
    await load({ force: true });
  } catch (e) {
    toast.error(`Couldn't set ${s.display_name} port`, (e as IpcError).message);
  } finally {
    end();
  }
}

// ── logs ──
const logsOpen = ref(false);
const logsTarget = ref<ServiceStatus | null>(null);

const logsTitle = computed(() => {
  const s = logsTarget.value;
  if (!s) return "Logs";
  return s.site ? `${s.display_name} · ${siteDomain(s, tld.value, sites.value)} logs` : `${s.display_name} logs`;
});

function openLogs(s: ServiceStatus): void {
  logsTarget.value = s;
  logsOpen.value = true;
}

// ── uninstall ──
const uninstallOpen = ref(false);
const uninstallTarget = ref<ServiceStatus | null>(null);
const uninstallVersionValue = ref("");
const uninstallPurge = ref(false);

const uninstallBody = computed(() => {
  const s = uninstallTarget.value;
  if (!s) return "";
  return purgeCopy(s.display_name, dbCounts.value[s.service] ?? null, uninstallPurge.value);
});

/** Opens on the next tick so the closing menu's focus restore doesn't fight the dialog. */
function openUninstall(s: ServiceStatus): void {
  uninstallTarget.value = s;
  uninstallVersionValue.value = uninstallVersion(s);
  uninstallPurge.value = false;
  void nextTick(() => {
    uninstallOpen.value = true;
  });
}

/** The target, version and purge choice are read once at confirm time, and
 *  the dialog's state is left alone afterwards, so a second confirm opened
 *  while this one runs keeps its own target. */
async function confirmUninstall(): Promise<void> {
  const s = uninstallTarget.value;
  const v = uninstallVersionValue.value;
  const purge = uninstallPurge.value;
  if (!s || !v) return;
  const end = beginBusy(`uninstall:${s.service}`);
  try {
    await uninstallService(s.service, v, purge);
    toast.success(`Uninstalled ${s.display_name} v${v}`);
    await Promise.all([load({ force: true }), refresh()]);
  } catch (e) {
    toast.error(`Uninstall of ${s.display_name}`, (e as IpcError).message);
    await load({ force: true });
  } finally {
    end();
  }
}

function onDatabasesChanged(): void {
  if (isRunningSql(selectedId.value)) void loadDatabases(selectedId.value);
}

onUnmounted(registerViewActions({ refresh: reloadAll }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Services" docs="/guide/services">
      <template #actions>
        <Button size="sm" :disabled="adding" data-testid="add-service" @click="openAdd">
          <Spinner v-if="adding" class="size-4" />
          <Plus v-else />
          Add service
        </Button>
      </template>
    </PageHeader>

    <div class="min-h-0 flex-1">
      <AsyncState :loading="loading" :error="error" :empty="!sections.length" pad="py-16" @retry="load">
        <template #empty>
          <div class="flex h-full flex-col items-center justify-center px-6 py-16 text-center" data-testid="services-empty">
            <span class="flex size-11 items-center justify-center rounded-xl border bg-card text-muted-foreground">
              <Database class="size-5" aria-hidden="true" />
            </span>
            <p class="mt-4 text-[15px] font-semibold">No services yet</p>
            <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">
              Add a database, cache or search engine. Yerd runs it on localhost and starts it with your sites.
            </p>
            <Button size="sm" class="mt-5" :disabled="adding" @click="openAdd"><Plus /> Add service</Button>
          </div>
        </template>

        <div class="flex h-full min-h-0" :data-layout="wide ? 'wide' : 'compact'">
          <div
            :class="wide ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-4' : 'flex min-w-0 flex-1 flex-col pl-4 pt-4'"
          >
            <ServicesList
              class="flex-1"
              :layout="wide ? 'list' : 'table'"
              :sections="sections"
              :selected="selectedId"
              :db-counts="dbCounts"
              :tld="tld"
              :busy="busyKeys"
              :sites="sites"
              @select="select"
            />
          </div>

          <div :class="wide ? 'min-w-0 flex-1 border-l' : 'hidden'" data-testid="inspector-column">
            <ServiceInspector
              :mode="wide ? 'panel' : 'drawer'"
              :shown="wide ? !!selected : drawerOpen && !!selected"
              :service="selected"
              :databases="selectedDatabases"
              :report="report ?? null"
              :sites="sites"
              :tld="tld"
              :busy="selectedBusy"
              @close="drawerOpen = false"
              @start="selected && doStart(selected)"
              @stop="selected && doStop(selected)"
              @restart="selected && doRestart(selected)"
              @logs="selected && openLogs(selected)"
              @edit-port="selected && openPort(selected)"
              @change-version="selected && openChange(selected)"
              @toggle-autostart="(v: boolean) => selected && doToggleAutostart(selected, v)"
              @uninstall="selected && openUninstall(selected)"
              @remove="selected && doRemove(selected)"
              @databases-changed="onDatabasesChanged"
              @open-site="openSite"
            />
            <p
              v-if="wide && !selected"
              class="flex h-full items-center justify-center text-[13px] text-faint"
            >
              Select a service to see its details.
            </p>
          </div>
        </div>
      </AsyncState>
    </div>

    <AddServiceDialog
      v-model:open="addOpen"
      :adding="adding"
      :services="services"
      :tld="tld"
      @add="confirmAdd"
    />

    <ServiceLogsDialog v-model:open="logsOpen" :service="logsTarget" :title="logsTitle" />

    <Modal v-model:open="versionOpen" :title="`Change ${versionTarget?.display_name ?? 'service'} version`">
      <div v-if="versionLoading" class="flex justify-center py-6"><Spinner class="size-5" /></div>
      <template v-else-if="versionOptions.length">
        <span class="text-sm font-medium">Version</span>
        <div class="mt-2">
          <Select v-model="selectedVersion" class="w-full" :options="versionSelectOptions" aria-label="version" />
        </div>
        <p
          :class="versionNote.blocked ? 'mt-2 text-xs text-destructive' : 'mt-2 text-xs text-muted-foreground'"
          data-testid="version-note"
        >
          {{ versionNote.text }}
        </p>
      </template>
      <p v-else class="py-2 text-sm text-muted-foreground">
        No other versions to switch to - the installed version is the only one offered
        for this platform, or the distribution couldn't be reached.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!versionOptions.length || versionNote.blocked" @click="confirmChange(close)">
          Switch
        </Button>
      </template>
    </Modal>

    <Modal v-model:open="portOpen" :title="`Edit ${portTarget?.display_name ?? 'service'} port`">
      <span class="text-sm font-medium">Port</span>
      <Input v-model="portValue" type="number" min="1" max="65535" class="mt-2" aria-label="Port" />
      <p class="mt-2 text-xs text-muted-foreground">
        The service binds 127.0.0.1 on this port. Restart the service to apply.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmPort(close)">Save</Button>
      </template>
    </Modal>

    <ConfirmDialog
      v-model:open="uninstallOpen"
      :title="`Uninstall ${uninstallTarget?.display_name ?? ''} ${uninstallVersionValue}?`"
      confirm-label="Uninstall"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmUninstall"
    >
      {{ uninstallBody }}
      <label
        class="mt-3 flex cursor-pointer items-center gap-2.5 rounded-md border bg-card px-3 py-3 text-[13px] text-foreground"
      >
        <input
          v-model="uninstallPurge"
          type="checkbox"
          class="size-4 accent-destructive"
          data-testid="uninstall-purge"
        />
        <span class="flex-1">Also delete the stored data</span>
        <span class="text-[11px] text-destructive">Can't be undone</span>
      </label>
    </ConfirmDialog>
  </div>
</template>
