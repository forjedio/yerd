<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Download, RefreshCw, Star } from "lucide-vue-next";

import ComingSoon from "@/components/ComingSoon.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  checkPhpUpdates,
  installPhp,
  IpcError,
  listPhp,
  setDefaultPhp,
  updatePhp,
} from "@/ipc/client";
import type { PhpPoolStatus, PhpUpdate, PhpVersion } from "@/ipc/types";
import { humaniseBytes, poolStateLabel, poolStateTone } from "@/lib/utils";

const toast = useToast();
const { report } = useDaemon();

const installed = ref<PhpVersion[]>([]);
const defaultVersion = ref<PhpVersion | null>(null);
const updates = ref<PhpUpdate[]>([]);
const loading = ref(true);
const busy = ref<string | null>(null); // a key naming the in-flight long op

// Live FPM state, keyed by version, from the shared status poll.
const poolByVersion = computed<Record<string, PhpPoolStatus>>(() => {
  const map: Record<string, PhpPoolStatus> = {};
  for (const p of report.value?.php ?? []) map[p.version] = p;
  return map;
});

const updateByVersion = computed<Record<string, PhpUpdate>>(() => {
  const map: Record<string, PhpUpdate> = {};
  for (const u of updates.value) map[u.version] = u;
  return map;
});

const hasUpdates = computed(() => updates.value.length > 0);

async function load(): Promise<void> {
  loading.value = true;
  try {
    const r = await listPhp();
    installed.value = r.installed;
    defaultVersion.value = r.default;
    updates.value = r.updates ?? [];
  } catch (e) {
    toast.error("Couldn't load PHP versions", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

async function refreshUpdates(): Promise<void> {
  busy.value = "refresh";
  try {
    const r = await checkPhpUpdates();
    installed.value = r.installed;
    defaultVersion.value = r.default;
    updates.value = r.updates ?? [];
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
    defaultVersion.value = v;
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
    await load();
  } catch (e) {
    toast.error("Update failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── install modal ──
const installOpen = ref(false);
const installLoading = ref(false);
const installOptions = ref<{ value: PhpVersion; label: string }[]>([]);
const selectedVersion = ref<PhpVersion>("");

// Open the modal and fetch the distribution's installable versions, hiding any
// already installed. Pre-selects the first so the Select (no placeholder) is
// always valid.
async function openInstall(): Promise<void> {
  installOpen.value = true;
  installLoading.value = true;
  installOptions.value = [];
  selectedVersion.value = "";
  try {
    const r = await availablePhp();
    const installedSet = new Set(r.installed);
    installOptions.value = r.available
      .filter((v) => !installedSet.has(v))
      .map((v) => ({ value: v, label: `PHP ${v}` }));
    selectedVersion.value = installOptions.value[0]?.value ?? "";
  } catch (e) {
    toast.error("Couldn't load installable versions", (e as IpcError).message);
  } finally {
    installLoading.value = false;
  }
}

async function confirmInstall(close: () => void): Promise<void> {
  const v = selectedVersion.value;
  if (!v) return;
  busy.value = "install";
  close();
  try {
    await installPhp(v);
    toast.success(`Installed PHP ${v}`);
    await load();
  } catch (e) {
    toast.error(`Install of PHP ${v} failed`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

onMounted(load);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="PHP" subtitle="Installed versions, updates, and the global default">
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="busy === 'refresh'"
          @click="refreshUpdates"
        >
          <Spinner v-if="busy === 'refresh'" class="size-4" />
          <RefreshCw v-else class="size-4" />
          Refresh updates
        </Button>
        <Button
          variant="outline"
          size="sm"
          :disabled="!hasUpdates || busy === 'update:all'"
          @click="doUpdate(null)"
        >
          <Spinner v-if="busy === 'update:all'" class="size-4" />
          Update all
        </Button>
        <Button size="sm" :disabled="busy === 'install'" @click="openInstall">
          <Spinner v-if="busy === 'install'" class="size-4" />
          <Download v-else class="size-4" />
          Install version
        </Button>
      </template>
    </PageHeader>

    <div class="flex-1 overflow-y-auto p-6">
      <div v-if="loading" class="flex justify-center py-16"><Spinner class="size-6" /></div>

      <div
        v-else-if="installed.length === 0"
        class="rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground"
      >
        No PHP versions installed yet. Use <strong>Install version</strong> to add one.
      </div>

      <table v-else class="w-full text-sm">
        <thead>
          <tr class="border-b text-left text-xs uppercase text-muted-foreground">
            <th class="py-2 pr-4 font-medium">Version</th>
            <th class="py-2 pr-4 font-medium">FPM</th>
            <th class="py-2 pr-4 font-medium">Patch</th>
            <th class="py-2 pr-4 font-medium">Memory</th>
            <th class="py-2 pr-4 font-medium">Update</th>
            <th class="py-2 pl-4 text-right font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="v in installed" :key="v" class="border-b last:border-0">
            <td class="py-3 pr-4">
              <div class="flex items-center gap-2">
                <span class="font-mono font-medium">PHP {{ v }}</span>
                <Badge v-if="v === defaultVersion" variant="secondary">
                  <Star class="size-3" /> default
                </Badge>
              </div>
            </td>
            <td class="py-3 pr-4">
              <StatusPill
                :tone="poolStateTone(poolByVersion[v]?.state)"
                :label="poolStateLabel(poolByVersion[v]?.state)"
              />
            </td>
            <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">
              {{ poolByVersion[v]?.installed_patch ?? "—" }}
            </td>
            <td class="py-3 pr-4 text-xs text-muted-foreground">
              {{ humaniseBytes(poolByVersion[v]?.rss_bytes) }}
            </td>
            <td class="py-3 pr-4">
              <Badge v-if="updateByVersion[v]" variant="warning">
                {{ updateByVersion[v].installed }} → {{ updateByVersion[v].latest }}
              </Badge>
              <span v-else class="text-xs text-muted-foreground">up to date</span>
            </td>
            <td class="py-3 pl-4">
              <div class="flex items-center justify-end gap-2">
                <Button
                  v-if="updateByVersion[v]"
                  variant="outline"
                  size="sm"
                  :disabled="busy === `update:${v}`"
                  @click="doUpdate(v)"
                >
                  <Spinner v-if="busy === `update:${v}`" class="size-4" />
                  Update
                </Button>
                <Button
                  v-if="v !== defaultVersion"
                  variant="ghost"
                  size="sm"
                  :disabled="busy === `default:${v}`"
                  @click="makeDefault(v)"
                >
                  Set default
                </Button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="mt-6 flex items-center gap-2 text-xs text-muted-foreground">
        <ComingSoon reason="Per-pool start/stop needs a daemon IPC — coming soon." pill>
          Start / stop pool
        </ComingSoon>
        <span>Updates are notify-only; nothing installs without your action.</span>
      </div>
    </div>

    <Modal v-model:open="installOpen" title="Install a PHP version">
      <div v-if="installLoading" class="flex justify-center py-6">
        <Spinner class="size-5" />
      </div>
      <template v-else-if="installOptions.length">
        <span class="text-sm font-medium">Version</span>
        <div class="mt-2">
          <Select
            class="w-full"
            :model-value="selectedVersion"
            :options="installOptions"
            aria-label="PHP version to install"
            @update:model-value="(v: PhpVersion) => (selectedVersion = v)"
          />
        </div>
        <p class="mt-2 text-xs text-muted-foreground">
          Downloads a prebuilt static build; this can take a few minutes with no
          progress bar (the daemon reports only on completion).
        </p>
      </template>
      <p v-else class="py-2 text-sm text-muted-foreground">
        No installable versions to add — every version offered by the
        distribution is already installed, or it couldn't be reached.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          :disabled="!installOptions.length || !selectedVersion"
          @click="confirmInstall(close)"
        >
          Install
        </Button>
      </template>
    </Modal>
  </div>
</template>
