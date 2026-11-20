<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import {
  Download,
  Hexagon,
  Info,
  MoreHorizontal,
  RefreshCw,
  Star,
  Trash2,
} from "lucide-vue-next";

import NodeInstallDialog from "@/components/node/NodeInstallDialog.vue";
import NodeInstallLogDialog from "@/components/node/NodeInstallLogDialog.vue";
import NodeOffDefaultCard from "@/components/node/NodeOffDefaultCard.vue";
import NodePickOrderCard from "@/components/node/NodePickOrderCard.vue";
import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import EmptyState from "@/components/ui/EmptyState.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  availableNode,
  installNodeWithProgress,
  IpcError,
  listNode,
  nodeSites,
  setNodeVersion,
  uninstallNode,
} from "@/ipc/client";
import type { NodeRelease } from "@/ipc/types";
import {
  clearedLabel,
  ltsLabel,
  majorOf,
  offDefaultSites,
  releaseTags,
  usedByCounts,
  usedByLabel,
} from "@/lib/node";

const toast = useToast();
const { report, refresh: refreshDaemon } = useDaemon();
const operations = useOperations();

const { data, loading, error, refresh: reloadNode, mutate } = useResource("node", listNode);
const {
  data: sitesData,
  loading: sitesLoading,
  error: sitesError,
  refresh: reloadSites,
} = useResource("node-sites", nodeSites);
const {
  data: majorsData,
  loading: majorsLoading,
  refreshing: majorsRefreshing,
  refresh: reloadMajors,
} = useResource("node-available", availableNode, { immediate: false });

const installed = computed<NodeRelease[]>(() => data.value ?? []);
const tld = computed(() => report.value?.tld ?? "test");
const defaultRelease = computed(() => installed.value.find((r) => r.default));
const usedBy = computed(() => usedByCounts(sitesData.value));
const offDefault = computed(() =>
  offDefaultSites(sitesData.value ?? [], defaultRelease.value?.version, tld.value),
);

watch(error, (e) => {
  if (e && !data.value) toast.error("Couldn't load Node versions", e.message);
});

/** A per-action key (`default:<v>` / `uninstall:<v>`) while a short mutation is in flight. */
const busy = ref<string | null>(null);

const installing = computed(() =>
  operations.active.value.some((o) => o.id.startsWith("node-install:")),
);

/** Every mutating action waits for a running install and for any other mutation. */
const locked = computed(() => installing.value || busy.value !== null);

const installOpen = ref(false);

function openInstall(): void {
  if (locked.value) return;
  installOpen.value = true;
}

const logOpen = ref(false);
const logTitle = ref("");
const logPhase = ref<"running" | "done" | "error">("running");
const installLog = ref<string[]>([]);

/**
 * Install `version` (a major or an exact version) as a streamed job tracked in
 * the global operations registry. Hiding the log leaves the job polling to its
 * end, so the toast and cache refresh still happen.
 */
async function install(version: string, update: boolean): Promise<void> {
  if (!version || locked.value) return;
  const opId = `node-install:${version}`;
  const label = update ? `Updating Node ${version}` : `Installing Node ${version}`;
  operations.begin({ id: opId, kind: "tool-install", label });
  logTitle.value = label;
  logPhase.value = "running";
  installLog.value = [];
  logOpen.value = true;
  try {
    await installNodeWithProgress(version, update, (lines) => {
      installLog.value.push(...lines);
      const latest = lines[lines.length - 1];
      if (latest) operations.update(opId, { detail: latest });
    });
    logPhase.value = "done";
    toast.success(update ? `Updated Node ${version}` : `Installed Node ${version}`);
  } catch (e) {
    logPhase.value = "error";
    toast.error(`Install of Node ${version} failed`, (e as IpcError).message);
  } finally {
    operations.end(opId);
    await Promise.all([reloadNode({ force: true }), refreshDaemon(), reloadSites({ force: true })]);
  }
}

/** Refetch the release index first: that refreshes the daemon's update cache the installed list reads. Only the Refresh button pays for this; installs refresh the cache in the daemon. */
async function refreshAll(): Promise<void> {
  await reloadMajors({ force: true });
  await Promise.all([reloadNode({ force: true }), refreshDaemon(), reloadSites({ force: true })]);
}

async function makeDefault(r: NodeRelease): Promise<void> {
  if (locked.value) return;
  busy.value = `default:${r.version}`;
  try {
    await setNodeVersion(r.version, null);
    toast.success(`Node ${r.version} is now the default`);
    await Promise.all([reloadNode({ force: true }), refreshDaemon(), reloadSites({ force: true })]);
  } catch (e) {
    toast.error("Couldn't set default", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

const uninstallOpen = ref(false);
const uninstallTarget = ref<NodeRelease | null>(null);
const uninstallIsLast = computed(() => installed.value.length === 1);
const clearedNames = computed(() =>
  (uninstallTarget.value?.clears_preferences ?? []).map((p) => clearedLabel(p, tld.value)),
);

/** Opened after the dropdown closes so its focus restore does not steal focus from the dialog. */
function openUninstall(r: NodeRelease): void {
  uninstallTarget.value = r;
  void nextTick(() => {
    uninstallOpen.value = true;
  });
}

async function confirmUninstall(): Promise<void> {
  const r = uninstallTarget.value;
  if (!r || locked.value) return;
  busy.value = `uninstall:${r.version}`;
  try {
    const next = await uninstallNode(r.version);
    mutate(() => next);
    toast.success(`Uninstalled Node ${r.version}`);
    await Promise.all([refreshDaemon(), reloadSites({ force: true })]);
  } catch (e) {
    toast.error(`Couldn't uninstall Node ${r.version}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    uninstallTarget.value = null;
  }
}

const CHIP = "inline-flex h-[19px] shrink-0 items-center gap-1 rounded px-[7px] text-[11px] font-medium";
const TH = "h-8 px-4 text-left align-middle text-[11px] font-medium uppercase tracking-[0.6px] text-faint";

onUnmounted(
  registerViewActions({
    create: () => openInstall(),
    refresh: () => void refreshAll(),
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Node" docs="/guide/node">
      <template #actions>
        <Button
          variant="outline"
          size="sm"
          :disabled="majorsRefreshing || majorsLoading"
          @click="refreshAll"
        >
          <Spinner v-if="majorsRefreshing" class="size-4" />
          <RefreshCw v-else class="size-4" />
          Refresh
        </Button>
        <Button size="sm" :disabled="locked" data-testid="node-install-open" @click="openInstall">
          <Spinner v-if="installing" class="size-4" />
          <Download v-else class="size-4" />
          Install
        </Button>
      </template>
    </PageHeader>

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <div
        class="flex items-start gap-2.5 rounded-[10px] border border-brand/30 bg-brand/10 px-3.5 py-3 text-[13px] leading-relaxed text-muted-foreground"
        data-testid="node-shim-note"
      >
        <Info class="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" />
        <p>
          The <code class="font-mono font-semibold text-foreground">node</code>,
          <code class="font-mono font-semibold text-foreground">npm</code> and
          <code class="font-mono font-semibold text-foreground">npx</code> commands on your PATH
          always use the global default. To use a project's version (from .nvmrc, .node-version or
          its saved preference) run
          <code class="font-mono font-semibold text-foreground">yerd exec npm …</code> inside the
          project.
        </p>
      </div>

      <div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_360px] xl:items-start">
        <section class="overflow-hidden rounded-[10px] border bg-card" data-testid="node-versions">
          <header class="px-4 py-3.5">
            <h3 class="text-sm font-semibold">Installed versions</h3>
            <p class="mt-0.5 text-xs text-faint">
              Releases install side by side. The default is what new shells, and sites without a
              project file or saved preference, use.
            </p>
          </header>

          <div v-if="loading" class="flex justify-center border-t py-12">
            <Spinner class="size-6" />
          </div>

          <div v-else-if="installed.length === 0" class="px-4 pb-4">
            <EmptyState
              :icon="Hexagon"
              title="No Node versions installed yet"
              description="Use Install to add a release and get node, npm and npx."
            />
          </div>

          <table v-else class="w-full table-fixed text-[13px]">
            <thead>
              <tr class="border-y">
                <th :class="TH">Version</th>
                <th :class="[TH, 'w-[150px]']">Used by</th>
                <th :class="[TH, 'w-[190px]']">Update</th>
                <th class="w-[72px]"><span class="sr-only">Actions</span></th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="r in installed"
                :key="r.version"
                class="h-[50px] border-b last:border-0"
                :data-testid="`node-row-${r.version}`"
              >
                <td class="px-4 py-2">
                  <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span class="font-mono text-[13px] font-semibold">Node {{ r.version }}</span>
                    <span
                      v-if="r.default"
                      :class="[CHIP, 'bg-brand/15 text-brand']"
                      data-testid="node-default-badge"
                    >
                      <Star class="size-3" aria-hidden="true" /> default
                    </span>
                    <span v-if="r.lts" :class="[CHIP, 'bg-secondary text-muted-foreground']">
                      {{ ltsLabel(r.codename) }}
                    </span>
                    <span
                      v-if="releaseTags(r, majorsData).current"
                      :class="[CHIP, 'bg-secondary text-muted-foreground']"
                      data-testid="node-tag-current"
                    >
                      Current
                    </span>
                    <span
                      v-if="releaseTags(r, majorsData).eol"
                      :class="[CHIP, 'bg-destructive/15 text-destructive']"
                      data-testid="node-tag-eol"
                    >
                      EOL
                    </span>
                  </div>
                </td>
                <td class="px-4 py-2 text-muted-foreground" data-testid="node-used-by">
                  {{ usedByLabel(usedBy[r.version]) }}
                </td>
                <td class="px-4 py-2">
                  <span v-if="r.update" :class="[CHIP, 'bg-warning/15 text-warning']">
                    ↑ {{ r.version }} → {{ r.update }}
                  </span>
                  <span v-else class="text-faint">—</span>
                </td>
                <td class="px-4 py-2">
                  <div class="flex items-center justify-end gap-2">
                    <Spinner v-if="busy?.endsWith(`:${r.version}`)" class="size-4" />
                    <DropdownMenu>
                      <DropdownMenuTrigger as-child>
                        <Button
                          variant="outline"
                          size="icon"
                          class="h-7 w-[30px]"
                          :aria-label="`Actions for Node ${r.version}`"
                        >
                          <MoreHorizontal class="size-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem
                          :disabled="locked || !r.update"
                          @select="install(majorOf(r.version), true)"
                        >
                          <Download class="size-4" />
                          {{ r.update ? `Update to ${r.update}` : "Update" }}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          :disabled="locked || r.default"
                          @select="makeDefault(r)"
                        >
                          <Star class="size-4" /> Make default
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                          :disabled="locked || r.uninstall_blocked"
                          :title="
                            r.uninstall_blocked
                              ? 'This is the global default - set another version as default first'
                              : undefined
                          "
                          @select="openUninstall(r)"
                        >
                          <Trash2 class="size-4" /> Uninstall…
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </section>

        <div class="grid gap-3 md:grid-cols-2 md:items-start xl:grid-cols-1 xl:gap-4">
          <NodePickOrderCard :default-version="defaultRelease?.version" />
          <NodeOffDefaultCard
            v-if="installed.length"
            :sites="offDefault"
            :loading="sitesLoading"
            :error="sitesError?.message ?? null"
          />
        </div>
      </div>
    </div>

    <NodeInstallDialog
      v-model:open="installOpen"
      :locked="locked"
      @install="(v: string) => install(v, false)"
    />

    <NodeInstallLogDialog
      v-model:open="logOpen"
      :title="logTitle"
      :phase="logPhase"
      :lines="installLog"
    />

    <ConfirmDialog
      v-model:open="uninstallOpen"
      title="Uninstall Node version"
      confirm-label="Uninstall"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmUninstall"
    >
      <span data-testid="node-uninstall-body">
        <span class="block">
          Remove
          <strong class="font-mono text-foreground">Node {{ uninstallTarget?.version }}</strong>
          and its files?
        </span>
        <span v-if="clearedNames.length" class="mt-2 block">
          Saved preference cleared for:
          <code class="font-mono font-semibold text-foreground">{{ clearedNames.join(", ") }}</code>
          - they will use their project file or the global default.
        </span>
        <span v-if="uninstallIsLast" class="mt-2 block">
          This is your only Node release, so <code>node</code>, <code>npm</code> and
          <code>npx</code> will leave your PATH.
        </span>
      </span>
    </ConfirmDialog>
  </div>
</template>
