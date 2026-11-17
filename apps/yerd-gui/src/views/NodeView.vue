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
  error: majorsError,
  refresh: reloadMajors,
} = useResource("node-available", availableNode);

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

/** The release line whose installed patch is `version` and has a newer patch available. */
function updateFor(version: string): MajorLine | undefined {
  return majors.value.find((l) => l.installed_latest === version && l.update_available);
}

const eolKnown = computed(() => majors.value.some((l) => typeof l.eol === "boolean"));

function ltsLabel(codename: string | undefined): string {
  return codename ? `LTS ${codename}` : "LTS";
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
    await Promise.all([
      reloadNode({ force: true }),
      reloadMajors({ force: true }),
      refreshDaemon(),
    ]);
  }
}

const exactVersion = ref("");

function installExact(): void {
  const v = exactVersion.value.trim();
  if (!v) return;
  exactVersion.value = "";
  void install(v, false);
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
    await Promise.all([reloadMajors({ force: true }), refreshDaemon()]);
  } catch (e) {
    toast.error(`Couldn't uninstall Node ${r.version}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    uninstallTarget.value = null;
  }
}

onUnmounted(
  registerViewActions({
    refresh: () => {
      void reloadNode();
      void reloadMajors();
    },
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Node"
      subtitle="Installed Node.js versions, updates, and the global default"
      docs="/guide/tooling#node-versions"
    />

    <div class="flex-1 space-y-8 overflow-y-auto p-6">
      <InfoBanner data-testid="node-shim-note">
        The <code>node</code>, <code>npm</code> and <code>npx</code> commands on your PATH always
        use the global default. To use a project's version (from <code>.nvmrc</code>,
        <code>.node-version</code> or its saved preference) run <code>yerd exec npm …</code>
        inside the project.
      </InfoBanner>

      <Card>
        <CardHeader>
          <CardTitle>Installed versions</CardTitle>
          <CardDescription>
            Releases install side by side. The default is what new shells and sites without a
            project file or saved preference use.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-12"><Spinner class="size-6" /></div>

          <EmptyState
            v-else-if="installed.length === 0"
            :icon="Hexagon"
            title="No Node versions installed yet"
            description="Install a release from the list below to get node, npm and npx."
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
                  <Badge v-if="updateFor(r.version)" variant="warning">
                    {{ r.version }} → {{ updateFor(r.version)?.latest }}
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
                          :disabled="locked || !updateFor(r.version)"
                          @select="install(String(updateFor(r.version)?.major ?? ''), true)"
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

      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Available releases</CardTitle>
            <CardDescription>
              The newest patch of each major line from nodejs.org. Update installs the newest
              patch beside the one you have.
            </CardDescription>
          </div>
          <Button
            variant="outline"
            size="sm"
            :disabled="majorsLoading"
            @click="reloadMajors({ force: true })"
          >
            <RefreshCw class="size-4" /> Refresh
          </Button>
        </CardHeader>

        <CardContent class="space-y-4">
          <InfoBanner v-if="majorsError" variant="destructive" data-testid="node-releases-error">
            <div class="flex flex-wrap items-center gap-2">
              <span>Couldn't load the Node release list: {{ majorsError.message }}</span>
              <Button variant="outline" size="sm" @click="reloadMajors({ force: true })">
                Retry
              </Button>
            </div>
          </InfoBanner>

          <div v-if="majorsLoading" class="flex justify-center py-8">
            <Spinner class="size-6" />
          </div>

          <template v-else-if="majors.length">
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                  <th class="py-2 pr-4 font-medium">Release</th>
                  <th class="py-2 pr-4 font-medium">Latest</th>
                  <th class="py-2 pl-4 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="l in majors"
                  :key="l.major"
                  class="border-b last:border-0"
                  :data-testid="`node-major-${l.major}`"
                >
                  <td class="py-3 pr-4">
                    <div class="flex items-center gap-2">
                      <span class="font-medium">Node {{ l.major }}</span>
                      <Badge v-if="l.codename" variant="outline">{{ ltsLabel(l.codename) }}</Badge>
                      <Badge v-if="l.eol === true" variant="warning">EOL</Badge>
                    </div>
                  </td>
                  <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">{{ l.latest }}</td>
                  <td class="py-3 pl-4">
                    <div class="flex items-center justify-end">
                      <Button
                        v-if="!l.installed_latest"
                        size="sm"
                        :disabled="locked"
                        @click="install(String(l.major), false)"
                      >
                        <Download class="size-3.5" /> Install
                      </Button>
                      <Button
                        v-else-if="l.update_available"
                        variant="outline"
                        size="sm"
                        :disabled="locked"
                        @click="install(String(l.major), true)"
                      >
                        <Download class="size-3.5" /> Update
                      </Button>
                      <span v-else class="text-xs text-muted-foreground">Installed</span>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
            <p v-if="!eolKnown" class="text-xs text-muted-foreground" data-testid="node-eol-unknown">
              End-of-life status unavailable.
            </p>
          </template>

          <div class="border-t pt-4">
            <label for="node-exact-version" class="text-sm font-medium">
              Install an exact version
            </label>
            <div class="mt-2 flex gap-2">
              <Input
                id="node-exact-version"
                v-model="exactVersion"
                placeholder="22.9.0"
                aria-label="Exact Node version"
                class="max-w-48 font-mono"
                @keydown.enter="installExact"
              />
              <Button
                size="sm"
                :disabled="locked || exactVersion.trim() === ''"
                data-testid="node-exact-install"
                @click="installExact"
              >
                <Download class="size-3.5" /> Install
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>

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
