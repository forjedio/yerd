<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { Download, Hexagon, MoreHorizontal, RefreshCw, Star, Trash2 } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
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
import EmptyState from "@/components/ui/EmptyState.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
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
  setNodeVersion,
  uninstallNode,
} from "@/ipc/client";
import type { MajorLine, NodeRelease } from "@/ipc/types";

const toast = useToast();
const { refresh: refreshDaemon } = useDaemon();
const operations = useOperations();

const { data, loading, error, refresh: reloadNode, mutate } = useResource("node", listNode);
const {
  data: majorsData,
  loading: majorsLoading,
  refreshing: majorsRefreshing,
  error: majorsError,
  refresh: reloadMajors,
} = useResource("node-available", availableNode, { immediate: false });

const installed = computed<NodeRelease[]>(() => data.value ?? []);
const majors = computed<MajorLine[]>(() => majorsData.value ?? []);

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

function majorOf(version: string): string {
  return version.split(".")[0] ?? "";
}

const eolKnown = computed(() => majors.value.some((l) => typeof l.eol === "boolean"));

function ltsLabel(codename: string | undefined): string {
  return codename ? `LTS ${codename}` : "LTS";
}

function lineLabel(l: MajorLine): string {
  const parts = [`Node ${l.major} (${l.latest})`];
  if (l.codename) parts.push(ltsLabel(l.codename));
  if (l.eol === true) parts.push("EOL");
  return parts.join(" - ");
}

/** Release lines with nothing installed yet, newest major first, as the daemon orders them. */
const uninstalledLines = computed(() => majors.value.filter((l) => !l.installed_latest));

/** End-of-life lines are hidden from the picker unless asked for. */
const showEol = ref(false);
const hasEolLines = computed(() => uninstalledLines.value.some((l) => l.eol === true));

const installOptions = computed(() =>
  uninstalledLines.value
    .filter((l) => showEol.value || l.eol !== true)
    .map((l) => ({ value: String(l.major), label: lineLabel(l) })),
);

/** The newest supported LTS line not yet installed, else the newest line on offer. */
function preferredMajor(): string {
  const lts = majors.value.find((l) => !l.installed_latest && l.codename && l.eol !== true);
  return lts ? String(lts.major) : (installOptions.value[0]?.value ?? "");
}

const installOpen = ref(false);
const selectedMajor = ref("");
const exactMode = ref(false);
const exactVersion = ref("");

const canInstall = computed(() =>
  exactMode.value ? exactVersion.value.trim() !== "" : selectedMajor.value !== "",
);

/** Keep the pick valid as the list loads or changes, without overriding a still-offered choice. */
watch(installOptions, (options) => {
  if (!options.some((o) => o.value === selectedMajor.value)) {
    selectedMajor.value = preferredMajor();
  }
});

/**
 * Open the install modal and refetch the release index (fetched only here and
 * on Refresh) so the picker offers the current newest patch of every line not
 * yet installed. The exact-version switch starts off unless there is no line
 * left to pick.
 */
async function openInstall(): Promise<void> {
  if (locked.value) return;
  exactMode.value = false;
  exactVersion.value = "";
  showEol.value = false;
  selectedMajor.value = preferredMajor();
  installOpen.value = true;
  await reloadMajors({ force: true });
  if (!uninstalledLines.value.length && !majorsError.value) exactMode.value = true;
}

function confirmInstall(): void {
  const v = exactMode.value ? exactVersion.value.trim() : selectedMajor.value;
  if (!v || locked.value) return;
  installOpen.value = false;
  void install(v, false);
}

const logOpen = ref(false);
const logTitle = ref("");
const logPhase = ref<"running" | "done" | "error">("running");
const installLog = ref<string[]>([]);
const logBox = ref<HTMLElement | null>(null);

async function appendLog(lines: string[]): Promise<void> {
  installLog.value.push(...lines);
  await nextTick();
  const el = logBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

/**
 * Install `version` (a major or an exact version) as a streamed job tracked in
 * the global operations registry. Closing the log modal leaves the job polling
 * to its end, so the toast and cache refresh still happen.
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
      void appendLog(lines);
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
    await Promise.all([reloadNode({ force: true }), refreshDaemon()]);
  }
}

/** Refetch the release index first: that refreshes the daemon's update cache the installed list reads. Only the Refresh button pays for this; installs refresh the cache in the daemon. */
async function refreshAll(): Promise<void> {
  await reloadMajors({ force: true });
  await Promise.all([reloadNode({ force: true }), refreshDaemon()]);
}

async function makeDefault(r: NodeRelease): Promise<void> {
  if (locked.value) return;
  busy.value = `default:${r.version}`;
  try {
    await setNodeVersion(r.version, null);
    toast.success(`Node ${r.version} is now the default`);
    await Promise.all([reloadNode({ force: true }), refreshDaemon()]);
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
  (uninstallTarget.value?.clears_preferences ?? []).map((p) => p.site ?? p.root),
);

/** Opened after the dropdown closes so its focus restore does not steal focus from the modal. */
function openUninstall(r: NodeRelease): void {
  uninstallTarget.value = r;
  void nextTick(() => {
    uninstallOpen.value = true;
  });
}

async function confirmUninstall(close: () => void): Promise<void> {
  const r = uninstallTarget.value;
  if (!r || locked.value) return;
  busy.value = `uninstall:${r.version}`;
  close();
  try {
    const next = await uninstallNode(r.version);
    mutate(() => next);
    toast.success(`Uninstalled Node ${r.version}`);
    await refreshDaemon();
  } catch (e) {
    toast.error(`Couldn't uninstall Node ${r.version}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    uninstallTarget.value = null;
  }
}

onUnmounted(
  registerViewActions({
    create: () => void openInstall(),
    refresh: () => void refreshAll(),
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Node"
      subtitle="Installed Node.js versions, updates, and the global default"
      docs="/guide/node"
    />

    <div class="flex-1 space-y-8 overflow-y-auto p-6">
      <InfoBanner data-testid="node-shim-note">
        The <code>node</code>, <code>npm</code> and <code>npx</code> commands on your PATH always
        use the global default. To use a project's version (from <code>.nvmrc</code>,
        <code>.node-version</code> or its saved preference) run <code>yerd exec npm …</code>
        inside the project.
      </InfoBanner>

      <Card>
        <CardHeader class="flex-row items-center justify-between gap-4 space-y-0">
          <div class="min-w-0 flex-1 space-y-1.5">
            <CardTitle>Installed versions</CardTitle>
            <CardDescription>
              Releases install side by side. The default is what new shells and sites without a
              project file or saved preference use.
            </CardDescription>
          </div>
          <div class="flex shrink-0 items-center gap-2">
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
          </div>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-12"><Spinner class="size-6" /></div>

          <EmptyState
            v-else-if="installed.length === 0"
            :icon="Hexagon"
            title="No Node versions installed yet"
            description="Use Install to add a release and get node, npm and npx."
          />

          <table v-else class="w-full text-sm">
            <thead>
              <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                <th class="py-2 pr-4 font-medium">Version</th>
                <th class="py-2 pr-4 font-medium">Update</th>
                <th class="py-2 pl-4 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="r in installed"
                :key="r.version"
                class="border-b last:border-0"
                :data-testid="`node-row-${r.version}`"
              >
                <td class="py-3 pr-4">
                  <div class="flex items-center gap-2">
                    <span class="font-mono font-medium">Node {{ r.version }}</span>
                    <Badge v-if="r.default" variant="secondary" data-testid="node-default-badge">
                      <Star class="size-3" /> default
                    </Badge>
                    <Badge v-if="r.lts" variant="outline">{{ ltsLabel(r.codename) }}</Badge>
                  </div>
                </td>
                <td class="py-3 pr-4">
                  <Badge v-if="r.update" variant="warning">
                    {{ r.version }} → {{ r.update }}
                  </Badge>
                  <span v-else class="text-xs text-muted-foreground">-</span>
                </td>
                <td class="py-3 pl-4">
                  <div class="flex items-center justify-end">
                    <Spinner v-if="busy?.endsWith(`:${r.version}`)" class="size-4" />
                    <DropdownMenu>
                      <DropdownMenuTrigger as-child>
                        <Button
                          variant="ghost"
                          size="icon"
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
                          <Download class="size-4" /> Update
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          :disabled="locked || r.default"
                          @select="makeDefault(r)"
                        >
                          <Star class="size-4" /> Set default
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

    <Modal v-model:open="installOpen" title="Install a Node version">
      <div class="space-y-4" data-testid="node-install-body">
        <InfoBanner v-if="majorsError" variant="destructive" data-testid="node-releases-error">
          <div class="flex flex-wrap items-center gap-2">
            <span>Couldn't load the Node release list: {{ majorsError.message }}</span>
            <Button variant="outline" size="sm" @click="reloadMajors({ force: true })">
              Retry
            </Button>
          </div>
        </InfoBanner>

        <div class="flex items-center gap-2 text-sm">
          <Switch
            v-model="exactMode"
            aria-labelledby="node-exact-mode-label"
            data-testid="node-exact-toggle"
          />
          <span
            id="node-exact-mode-label"
            class="cursor-pointer"
            @click="exactMode = !exactMode"
          >
            Install an exact version
          </span>
        </div>

        <div v-if="!exactMode && hasEolLines" class="flex items-center gap-2 text-sm">
          <Switch
            v-model="showEol"
            aria-labelledby="node-eol-mode-label"
            data-testid="node-eol-toggle"
          />
          <span id="node-eol-mode-label" class="cursor-pointer" @click="showEol = !showEol">
            Show end-of-life versions
          </span>
        </div>

        <div class="border-t pt-4">
          <span class="text-sm font-medium">Version</span>
          <div class="mt-2">
            <Input
              v-if="exactMode"
              v-model="exactVersion"
              placeholder="22.9.0"
              aria-label="Exact Node version"
              class="font-mono"
              @keydown.enter="confirmInstall"
            />
            <div v-else-if="majorsLoading && !majorsData" class="flex justify-center py-4">
              <Spinner class="size-5" />
            </div>
            <Select
              v-else-if="installOptions.length"
              v-model="selectedMajor"
              class="w-full"
              :options="installOptions"
              aria-label="Node release to install"
            />
            <p v-else class="text-sm text-muted-foreground" data-testid="node-install-none">
              <template v-if="hasEolLines">
                Every supported release line is already installed. Show end-of-life versions, or
                use an exact version to add an older patch.
              </template>
              <template v-else>
                Every listed release line is already installed. Use an exact version to add an
                older patch.
              </template>
            </p>
          </div>
          <p class="mt-2 text-xs text-muted-foreground">
            <template v-if="exactMode">
              Any release from major 9 up, for example <code>22.9.0</code> or <code>18</code>.
            </template>
            <template v-else>
              Installs the newest patch of the chosen line beside any you already have.
            </template>
          </p>
          <p
            v-if="!exactMode && majors.length && !eolKnown"
            class="mt-1 text-xs text-muted-foreground"
            data-testid="node-eol-unknown"
          >
            End-of-life status unavailable.
          </p>
        </div>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          :disabled="!canInstall || locked"
          data-testid="node-install-submit"
          @click="confirmInstall"
        >
          Install
        </Button>
      </template>
    </Modal>

    <Modal v-model:open="logOpen" :title="logTitle" size="lg">
      <pre
        ref="logBox"
        class="h-72 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ installLog.join("\n") || "Starting…" }}</pre>
      <p v-if="logPhase === 'running'" class="mt-2 text-xs text-muted-foreground">
        You can close this window; the install keeps running in the background.
      </p>
      <template #footer="{ close }">
        <Spinner v-if="logPhase === 'running'" class="size-4" />
        <Button @click="close">{{ logPhase === "running" ? "Hide" : "Done" }}</Button>
      </template>
    </Modal>

    <Modal v-model:open="uninstallOpen" title="Uninstall Node version">
      <div class="space-y-3 text-sm text-muted-foreground" data-testid="node-uninstall-body">
        <p>
          Remove <strong class="font-mono text-foreground">Node {{ uninstallTarget?.version }}</strong>
          and its files?
        </p>
        <p v-if="clearedNames.length">
          Saved preference cleared for: {{ clearedNames.join(", ") }} - they will use their
          project file or the global default.
        </p>
        <p v-if="uninstallIsLast">
          This is your only Node release, so <code>node</code>, <code>npm</code> and
          <code>npx</code> will leave your PATH.
        </p>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          variant="destructive"
          data-testid="node-uninstall-confirm"
          :disabled="locked"
          @click="confirmUninstall(close)"
        >
          Uninstall
        </Button>
      </template>
    </Modal>
  </div>
</template>
