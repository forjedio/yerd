<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import { CheckCircle2, Folder, RefreshCw, Trash2, Wrench } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import AvailableToolCard from "@/components/tooling/AvailableToolCard.vue";
import ToolInstallLogDialog from "@/components/tooling/ToolInstallLogDialog.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import EmptyState from "@/components/ui/EmptyState.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  installToolStreamed,
  IpcError,
  listTools,
  openPath,
  pollJobToEnd,
  uninstallTool,
} from "@/ipc/client";
import type { ToolStatus } from "@/ipc/types";
import {
  commandList,
  needsComposer,
  onPathNote,
  splitTools,
  statusText,
  toolDescription,
} from "@/lib/tooling";

const toast = useToast();
const { refresh: refreshDaemon } = useDaemon();
const operations = useOperations();

const { data, loading, error, refresh: reload } = useResource("tools", listTools);

watch(error, (e) => {
  if (e && !data.value) toast.error("Couldn't load tools", e.message);
});

const all = computed<ToolStatus[]>(() => data.value ?? []);
const split = computed(() => splitTools(all.value));

const OP_PREFIX = "tool-install:";

const installingIds = computed(() =>
  operations.active.value
    .filter((o) => o.id.startsWith(OP_PREFIX))
    .map((o) => o.id.slice(OP_PREFIX.length)),
);

/** `uninstall:<id>` while an uninstall is in flight. */
const busy = ref<string | null>(null);

/** Every mutating action waits for a running install and for an uninstall. */
const locked = computed(() => installingIds.value.length > 0 || busy.value !== null);

const logs = ref<Record<string, string[]>>({});
const failures = ref<Record<string, string>>({});
const titles = ref<Record<string, string>>({});

const logOpen = ref(false);
const logToolId = ref<string | null>(null);

const logTool = computed(() => all.value.find((t) => t.id === logToolId.value));

const logPhase = computed<"running" | "done" | "error">(() => {
  const id = logToolId.value;
  if (id && installingIds.value.includes(id)) return "running";
  if (id && failures.value[id] !== undefined) return "error";
  return "done";
});

const logTitle = computed(() => {
  const id = logToolId.value;
  if (!id) return "";
  return titles.value[id] ?? `Installing ${logTool.value?.display_name ?? id}`;
});

const logLines = computed<string[]>(() => {
  const id = logToolId.value;
  if (!id) return [];
  const lines = logs.value[id] ?? [];
  if (lines.length) return lines;
  const detail = operations.get(OP_PREFIX + id)?.detail;
  return detail ? [detail] : [];
});

/** The latest streamed line for a card, falling back to the operation's detail after a remount. */
function detailFor(id: string): string | undefined {
  const lines = logs.value[id] ?? [];
  return lines[lines.length - 1] ?? operations.get(OP_PREFIX + id)?.detail;
}

function cardState(id: string): "idle" | "installing" | "failed" {
  if (installingIds.value.includes(id)) return "installing";
  if (failures.value[id] !== undefined) return "failed";
  return "idle";
}

const COMPOSER_REQUIRED_HINT =
  "Yerd's own Composer is required to build this tool - install Yerd's Composer first.";

function updateTitle(t: ToolStatus): string {
  if (needsComposer(t, all.value)) return COMPOSER_REQUIRED_HINT;
  return t.update_available ? `Update to ${t.update_available}` : "Up to date";
}

/**
 * Install (or, for an installed tool with a known update, update) `t` as a
 * streamed job tracked in the global operations registry. The poll runs to the
 * job's end even after the log closes or the view unmounts, so the toast and
 * the shared list refresh still happen.
 */
async function install(t: ToolStatus, update: boolean): Promise<void> {
  if (locked.value) return;
  const id = t.id;
  const opId = OP_PREFIX + id;
  const label = `${update ? "Updating" : "Installing"} ${t.display_name}`;
  operations.begin({ id: opId, kind: "tool-install", label });
  failures.value = Object.fromEntries(
    Object.entries(failures.value).filter(([k]) => k !== id),
  );
  logs.value = { ...logs.value, [id]: [] };
  titles.value = { ...titles.value, [id]: label };
  logToolId.value = id;
  if (update) logOpen.value = true;
  try {
    const jobId = await installToolStreamed(id);
    const final = await pollJobToEnd(
      jobId,
      (lines) => {
        logs.value = { ...logs.value, [id]: [...(logs.value[id] ?? []), ...lines] };
        const latest = lines
          .map(statusText)
          .filter((l): l is string => l !== null)
          .pop();
        if (latest) operations.update(opId, { detail: latest });
      },
      undefined,
      500,
      5,
    );
    if (final.state !== "succeeded") {
      throw new IpcError(final.error || `install ${final.state}`);
    }
    await reload({ force: true });
    const version = all.value.find((x) => x.id === id)?.version;
    toast.success(
      `${update ? "Updated" : "Installed"} ${t.display_name}${version ? ` ${version}` : ""}`,
      onPathNote(t.binaries),
    );
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    failures.value = { ...failures.value, [id]: message };
    toast.error(`Install of ${t.display_name} failed`, message);
  } finally {
    operations.end(opId);
    await Promise.all([reload({ force: true }), refreshDaemon()]);
  }
}

function installComposer(): void {
  const composer = all.value.find((t) => t.id === "composer");
  if (composer) void install(composer, false);
}

function showLog(id: string): void {
  logToolId.value = id;
  logOpen.value = true;
}

async function reveal(path: string): Promise<void> {
  try {
    await openPath(path);
  } catch (e) {
    toast.error(`Couldn't reveal ${path}`, e instanceof Error ? e.message : String(e));
  }
}

const uninstallOpen = ref(false);
const uninstallTarget = ref<ToolStatus | null>(null);

function openUninstall(t: ToolStatus): void {
  if (locked.value) return;
  uninstallTarget.value = t;
  uninstallOpen.value = true;
}

async function confirmUninstall(): Promise<void> {
  const t = uninstallTarget.value;
  if (!t || locked.value) return;
  busy.value = `uninstall:${t.id}`;
  try {
    await uninstallTool(t.id);
    failures.value = Object.fromEntries(
      Object.entries(failures.value).filter(([k]) => k !== t.id),
    );
    toast.success(`Removed ${t.display_name}`, "Its commands are off your PATH");
    await Promise.all([reload({ force: true }), refreshDaemon()]);
  } catch (e) {
    toast.error(`Uninstall of ${t.display_name} failed`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

const LABEL = "text-[11px] font-semibold uppercase tracking-[0.6px] text-faint";
const CHIP = "inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1.5 font-mono text-[11px] font-medium";
const UPDATE_CHIP =
  "inline-flex h-5 shrink-0 items-center rounded border border-warning/40 bg-warning/15 px-1.5 font-mono text-[11px] font-medium text-warning";
const CMD =
  "inline-flex h-[18px] items-center rounded bg-background px-1.5 font-mono text-[11px] text-muted-foreground";

onUnmounted(registerViewActions({ refresh: () => void reload({ force: true }) }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Tooling" docs="/guide/tooling" />

    <div class="flex-1 overflow-y-auto p-6">
      <div v-if="loading" class="flex justify-center py-12">
        <Spinner class="size-6" />
      </div>

      <div
        v-else
        class="grid gap-7 xl:grid-cols-[minmax(0,1fr)_286px] xl:items-start xl:gap-5"
      >
        <section data-testid="tooling-installed">
          <h2 :class="[LABEL, 'mb-3 flex gap-2']">
            <span>Installed</span>
            <span data-testid="tooling-installed-count">{{ split.installed.length }}</span>
          </h2>

          <div
            v-if="split.installed.length"
            class="divide-y overflow-hidden rounded-[10px] border bg-card"
          >
            <div
              v-for="t in split.installed"
              :key="t.id"
              class="flex min-h-[88px] items-center gap-5 px-5 py-3"
              :data-testid="`tool-row-${t.id}`"
            >
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="text-sm font-semibold">{{ t.display_name }}</span>
                  <span :class="[CHIP, 'bg-success/15 text-success']">
                    <span class="size-1.5 rounded-full bg-success" aria-hidden="true" />
                    {{ t.version ?? "installed" }}
                  </span>
                  <span
                    v-if="t.update_available"
                    :class="UPDATE_CHIP"
                    :title="`Version ${t.update_available} is available`"
                    :data-testid="`tool-update-${t.id}`"
                  >
                    ↑ {{ t.update_available }}
                  </span>
                </div>
                <p class="mt-1.5 text-xs text-muted-foreground">{{ toolDescription(t.id) }}</p>
                <div v-if="t.binaries.length" class="mt-1.5 flex flex-wrap gap-1.5">
                  <code v-for="b in t.binaries" :key="b" :class="CMD">{{ b }}</code>
                </div>
              </div>
              <div class="flex shrink-0 items-center gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="locked || needsComposer(t, all) || !t.update_available"
                  :title="updateTitle(t)"
                  @click="install(t, true)"
                >
                  <Spinner v-if="installingIds.includes(t.id)" class="size-4" />
                  <RefreshCw v-else /> Update
                </Button>
                <Button
                  variant="outline"
                  size="icon"
                  class="size-8"
                  :disabled="locked"
                  :aria-label="`Uninstall ${t.display_name}`"
                  title="Uninstall"
                  @click="openUninstall(t)"
                >
                  <Spinner v-if="busy === `uninstall:${t.id}`" class="size-4" />
                  <Trash2 v-else class="!size-3.5" />
                </Button>
              </div>
            </div>
          </div>

          <EmptyState
            v-else
            :icon="Wrench"
            title="No tools installed yet"
            description="Pick one from Available and it goes on your PATH beside PHP."
          />

          <p class="mt-3 flex items-center gap-2 text-xs text-faint">
            <Folder class="size-3.5 shrink-0" aria-hidden="true" />
            Tools install into Yerd's data folder and go on your PATH beside PHP. Node.js is
            managed from the Node page.
          </p>
        </section>

        <section data-testid="tooling-available">
          <h2 :class="[LABEL, 'mb-3 flex gap-2']">
            <span>Available</span>
            <span data-testid="tooling-available-count">{{ split.available.length }}</span>
          </h2>

          <div
            v-if="split.available.length"
            class="grid gap-3 md:grid-cols-2 md:items-start xl:grid-cols-1"
          >
            <AvailableToolCard
              v-for="t in split.available"
              :key="t.id"
              :tool="t"
              :state="cardState(t.id)"
              :blocked="needsComposer(t, all)"
              :locked="locked"
              :detail="detailFor(t.id)"
              :error="failures[t.id]"
              @install="install(t, false)"
              @install-composer="installComposer"
              @show-log="showLog(t.id)"
              @reveal="reveal"
            />
          </div>

          <EmptyState
            v-else
            :icon="CheckCircle2"
            title="Everything's installed"
            description="New tools show up here when Yerd adds them."
          />
        </section>
      </div>
    </div>

    <ToolInstallLogDialog
      v-model:open="logOpen"
      :title="logTitle"
      :phase="logPhase"
      :lines="logLines"
    />

    <ConfirmDialog
      v-model:open="uninstallOpen"
      :title="`Uninstall ${uninstallTarget?.display_name ?? 'tool'}?`"
      confirm-label="Uninstall"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmUninstall"
    >
      <span data-testid="tool-uninstall-body">
        <code class="font-mono text-foreground">{{
          commandList(uninstallTarget?.binaries ?? [])
        }}</code>
        {{ (uninstallTarget?.binaries.length ?? 0) > 1 ? "leave" : "leaves" }} your PATH. Your
        projects aren't touched, and you can reinstall it any time.
      </span>
    </ConfirmDialog>
  </div>
</template>
