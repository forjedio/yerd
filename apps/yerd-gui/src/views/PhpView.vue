<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { Download, RefreshCw, Trash2, Undo2 } from "lucide-vue-next";

import AddExtensionModal from "@/components/AddExtensionModal.vue";
import InstallPhpDialog, { type InstallRequest } from "@/components/php/InstallPhpDialog.vue";
import InstallProgressDialog from "@/components/php/InstallProgressDialog.vue";
import PhpInspector from "@/components/php/PhpInspector.vue";
import PhpVersionsList, { type PhpSelection } from "@/components/php/PhpVersionsList.vue";
import PageHeader from "@/components/PageHeader.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  checkPhpUpdates,
  installPhpWithProgress,
  IpcError,
  listPhp,
  listPhpExtensions,
  type PhpExtensionsMap,
  restartAllPhp,
  restartPhp,
  setDefaultPhp,
  uninstallPhp,
  updatePhp,
} from "@/ipc/client";
import type { PhpPoolStatus, PhpUpdate, PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import { listVersions } from "@/lib/php";
import { comparePhpVersions } from "@/lib/phpVersion";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { joinWithAnd } from "@/lib/utils";

const toast = useToast();
const { report, refresh } = useDaemon();
const operations = useOperations();
const wide = useWideLayout();

const { data, loading, error: resourceError, refresh: reloadPhp, mutate } = useResource("php", listPhp);
const installed = computed<PhpVersion[]>(() => data.value?.installed ?? []);
const defaultVersion = computed<PhpVersion | null>(() => data.value?.default ?? null);
const updates = computed<PhpUpdate[]>(() => data.value?.updates ?? []);
/** A key naming the in-flight version-level action, e.g. `restart:8.4`. */
const busy = ref<string | null>(null);

/** Only a cold-load failure replaces the screen; a failed revalidation over
 *  cached data keeps the list and inspector (and their staged edits) mounted. */
const error = computed(() => (data.value ? null : (resourceError.value?.message ?? null)));

/** Installs live in the global operations registry so they outlive this view. */
const installing = computed(() => operations.active.value.some((o) => o.kind === "php-install"));
const installDetail = computed(
  () => operations.active.value.find((o) => o.kind === "php-install")?.detail ?? "",
);

const poolByVersion = computed<Record<string, PhpPoolStatus>>(() => {
  const map: Record<string, PhpPoolStatus> = {};
  for (const p of report.value?.php ?? []) map[p.version] = p;
  return map;
});

const {
  data: extData,
  loading: extLoading,
  error: extError,
  mutate: mutateExts,
} = useResource("php-extensions", listPhpExtensions);

watch(extError, (e) => {
  if (e && !extData.value) toast.error("Couldn't load extensions", e.message);
});

const versions = computed<PhpVersion[]>(() =>
  listVersions(installed.value, Object.keys(extData.value ?? {})),
);

function onVersionConfigUpdated(r: PhpVersionsResponse): void {
  mutate(() => r);
  void reloadPhp({ force: true });
}

function onExtensionsUpdated(map: PhpExtensionsMap): void {
  mutateExts(() => map);
}

// ── selection, drawer and unsaved edits ──
const selected = ref<PhpSelection>("shared");
const drawerOpen = ref(false);
const dirtyByVersion = ref<Record<string, boolean>>({});
const sharedDirty = ref(false);

let seeded = false;

/** Starts on the default (else the newest version, else Shared defaults) once the
 *  list loads, then re-points only when the selected version disappears. */
watch(
  [versions, defaultVersion, data],
  ([vs, def, loaded]) => {
    if (!loaded) return;
    if (selected.value === "shared" ? seeded : vs.includes(selected.value)) return;
    seeded = true;
    selected.value = def && vs.includes(def) ? def : (vs[0] ?? "shared");
  },
  { immediate: true },
);

const dirtyFlags = computed<Record<string, boolean>>(() => ({
  ...dirtyByVersion.value,
  shared: sharedDirty.value,
}));

/** The unsaved items, newest version first and Shared defaults last. */
const dirtyNames = computed<string[]>(() => {
  const names = Object.entries(dirtyByVersion.value)
    .filter(([, d]) => d)
    .map(([v]) => v)
    .sort((a, b) => comparePhpVersions(b, a))
    .map((v) => `PHP ${v}`);
  if (sharedDirty.value) names.push("Shared defaults");
  return names;
});

const discardOpen = ref(false);

const discardBody = computed(() => {
  return `Your edits to ${joinWithAnd(dirtyNames.value)} haven't been saved.`;
});

function select(key: PhpSelection): void {
  selected.value = key;
  if (!wide.value) drawerOpen.value = true;
}

/** A drawer close discards staged edits (its content unmounts), so ask first. */
function requestClose(): void {
  if (dirtyNames.value.length > 0) {
    discardOpen.value = true;
    return;
  }
  drawerOpen.value = false;
}

function confirmDiscard(): void {
  drawerOpen.value = false;
  dirtyByVersion.value = {};
  sharedDirty.value = false;
}

watch(wide, (isWide) => {
  drawerOpen.value = !isWide && dirtyNames.value.length > 0;
});

function onDirty(v: PhpVersion, d: boolean): void {
  dirtyByVersion.value = { ...dirtyByVersion.value, [v]: d };
}

// ── version actions ──
/**
 * Check for patch updates. The daemon answers with the full PHP state, so it
 * replaces the cached view whole and settings such as Xdebug stay current.
 */
async function refreshUpdates(): Promise<void> {
  busy.value = "refresh";
  try {
    const r = await checkPhpUpdates();
    mutate(() => ({ ...r, updates: r.updates ?? [] }));
    toast.success(
      "Update check complete",
      r.updates?.length ? `${r.updates.length} update(s) available` : "All up to date",
    );
  } catch (e) {
    toast.error("Update check failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function makeDefault(v: PhpVersion): Promise<void> {
  busy.value = `default:${v}`;
  try {
    await setDefaultPhp(v);
    mutate((cur) => (cur ? { ...cur, default: v } : cur));
    toast.success(`PHP ${v} is now the default`);
  } catch (e) {
    toast.error("Couldn't set default", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doUpdate(v: PhpVersion | null): Promise<void> {
  busy.value = v ? `update:${v}` : "update:all";
  try {
    await updatePhp(v);
    toast.success(v ? `Updated PHP ${v}` : "Updated all PHP versions");
    await Promise.all([reloadPhp({ force: true }), refresh()]);
  } catch (e) {
    toast.error("Update failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

const anyRunning = computed(() =>
  (report.value?.php ?? []).some((p) => p.state === "running" || p.state === "failed"),
);

async function doRestart(v: PhpVersion): Promise<void> {
  busy.value = `restart:${v}`;
  try {
    await restartPhp(v);
    toast.success(`Restarted PHP ${v}`);
    await refresh();
  } catch (e) {
    toast.error(`Couldn't restart PHP ${v}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doRestartAll(): Promise<void> {
  if (!anyRunning.value) {
    toast.info("No running pools to restart");
    return;
  }
  busy.value = "restart:all";
  try {
    await restartAllPhp();
    toast.success("Restarted all running pools");
    await refresh();
  } catch (e) {
    toast.error("Couldn't restart pools", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── uninstall ──
const uninstallOpen = ref(false);
const uninstallTarget = ref<PhpVersion | null>(null);

/** Opens on the next tick so the closing menu's focus restore doesn't fight the dialog. */
function openUninstall(v: PhpVersion): void {
  uninstallTarget.value = v;
  void nextTick(() => {
    uninstallOpen.value = true;
  });
}

async function confirmUninstall(): Promise<void> {
  const v = uninstallTarget.value;
  if (!v) return;
  busy.value = `uninstall:${v}`;
  try {
    await uninstallPhp(v);
    toast.success(`Uninstalled PHP ${v}`);
    await reloadPhp({ force: true });
  } catch (e) {
    toast.error(`Couldn't uninstall PHP ${v}`, (e as IpcError).message);
    await reloadPhp({ force: true });
  } finally {
    busy.value = null;
    uninstallTarget.value = null;
  }
}

// ── extensions ──
const addExtOpen = ref(false);
const addExtVersion = ref<PhpVersion>("");

function openAddExtension(v: PhpVersion): void {
  addExtVersion.value = v;
  addExtOpen.value = true;
}

// ── install ──
const installOpen = ref(false);
const installProgressOpen = ref(false);
const installPhase = ref<"running" | "done" | "error">("running");
const installError = ref("");
const installTarget = ref<PhpVersion>("");

function openInstall(): void {
  installOpen.value = true;
}

/**
 * Install a version with live progress. Only one PHP install runs at a time, so
 * this no-ops while any `php-install` operation is active. The progress dialog
 * can be closed at any point; the install carries on, the sidebar keeps showing
 * it and a toast reports the outcome. On success it refreshes the version list
 * and the status poll so the new row shows its patch and state at once.
 */
async function confirmInstall({ version: v, legacy }: InstallRequest): Promise<void> {
  if (!v || installing.value) return;
  const opId = `php-install:${v}`;
  installTarget.value = v;
  installPhase.value = "running";
  installError.value = "";
  operations.begin({ id: opId, kind: "php-install", label: `Installing PHP ${v}` });
  installOpen.value = false;
  installProgressOpen.value = true;
  try {
    await installPhpWithProgress(
      v,
      (lines) => {
        const latest = lines[lines.length - 1];
        if (latest) operations.update(opId, { detail: latest });
      },
      legacy,
    );
    installPhase.value = "done";
    toast.success(`Installed PHP ${v}`);
    await Promise.all([reloadPhp({ force: true }), refresh()]);
  } catch (e) {
    installPhase.value = "error";
    installError.value = (e as IpcError).message;
    toast.error(`Install of PHP ${v} failed`, (e as IpcError).message);
  } finally {
    operations.end(opId);
  }
}

onUnmounted(
  registerViewActions({
    create: () => openInstall(),
    refresh: () => void reloadPhp(),
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="PHP" docs="/guide/php-versions">
      <template #actions>
        <Button variant="outline" size="sm" :disabled="busy === 'refresh'" @click="refreshUpdates">
          <Spinner v-if="busy === 'refresh'" class="size-4" />
          <RefreshCw v-else />
          Check for updates
        </Button>
        <Button size="sm" :disabled="installing" data-testid="install-php" @click="openInstall">
          <Spinner v-if="installing" class="size-4" />
          <Download v-else />
          Install PHP
        </Button>
      </template>
    </PageHeader>

    <div class="min-h-0 flex-1">
      <AsyncState :loading="loading" :error="error" pad="py-16" @retry="reloadPhp">
        <div class="flex h-full min-h-0" :data-layout="wide ? 'wide' : 'compact'">
          <div
            :class="wide ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-4' : 'flex min-w-0 flex-1 flex-col pl-4 pt-4'"
          >
            <PhpVersionsList
              class="flex-1"
              :layout="wide ? 'list' : 'table'"
              :versions="versions"
              :installed="installed"
              :default-version="defaultVersion"
              :selected="selected"
              :data="data"
              :extensions="extData"
              :pools="poolByVersion"
              :updates="updates"
              :dirty="dirtyFlags"
              :busy="busy"
              @select="select"
              @install="openInstall"
              @update="doUpdate"
            />
          </div>

          <div :class="wide ? 'min-w-0 flex-1 border-l' : 'hidden'" data-testid="inspector-column">
            <PhpInspector
              :mode="wide ? 'panel' : 'drawer'"
              :shown="wide || drawerOpen"
              :selected="selected"
              :versions="versions"
              :installed="installed"
              :default-version="defaultVersion"
              :data="data"
              :extensions="extData"
              :extensions-loading="extLoading"
              :pools="poolByVersion"
              :updates="updates"
              :busy="busy"
              :any-running="anyRunning"
              @close="requestClose"
              @restart="doRestart"
              @update="doUpdate"
              @make-default="makeDefault"
              @uninstall="openUninstall"
              @restart-all="doRestartAll"
              @updated="onVersionConfigUpdated"
              @extensions-updated="onExtensionsUpdated"
              @request-add-extension="openAddExtension"
              @dirty="onDirty"
              @shared-dirty="(d: boolean) => (sharedDirty = d)"
            />
          </div>
        </div>
      </AsyncState>
    </div>

    <AddExtensionModal
      v-model:open="addExtOpen"
      :version="addExtVersion"
      @added="onExtensionsUpdated"
    />

    <InstallPhpDialog v-model:open="installOpen" :installing="installing" @install="confirmInstall" />

    <InstallProgressDialog
      v-model:open="installProgressOpen"
      :version="installTarget"
      :phase="installPhase"
      :detail="installDetail"
      :error="installError"
    />

    <ConfirmDialog
      v-model:open="uninstallOpen"
      :title="`Uninstall PHP ${uninstallTarget ?? ''}?`"
      confirm-label="Uninstall"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmUninstall"
    >
      Its pool stops and the build is deleted. Yerd won't uninstall a version a site still uses,
      or the default while other versions are installed.
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="discardOpen"
      title="Discard unsaved changes?"
      confirm-label="Discard"
      tone="danger"
      :icon="Undo2"
      @confirm="confirmDiscard"
    >
      {{ discardBody }}
    </ConfirmDialog>
  </div>
</template>
