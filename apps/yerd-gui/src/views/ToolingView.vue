<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { Download, RefreshCw, Trash2 } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  installToolStreamed,
  IpcError,
  listTools,
  pollJobToEnd,
  uninstallTool,
} from "@/ipc/client";
import type { ToolStatus } from "@/ipc/types";

const toast = useToast();
const { refresh } = useDaemon();

const tools = ref<ToolStatus[]>([]);
const loading = ref(true);
// Which tool has a long-running op in flight, e.g. "install:node".
const busy = ref<string | null>(null);

/** Short description per tool, keyed by wire id. */
const TOOL_HINTS: Record<string, string> = {
  composer: "PHP dependency manager.",
  node: "Node.js runtime for building frontend assets (npm, npx).",
  bun: "Fast all-in-one JavaScript runtime and package manager.",
  laravel: "The laravel new installer for scaffolding new Laravel apps. Needs Composer.",
};

// Managed-only: building the managed Laravel installer requires Yerd's own
// Composer (an external Composer can't build it), so this gate ignores `external`.
const composerInstalled = computed(() =>
  tools.value.some((t) => t.id === "composer" && t.installed),
);

const COMPOSER_REQUIRED_HINT =
  "Yerd's own Composer is required to build the Laravel installer.";

/** The Laravel installer is built via Composer, so it can't install without it. */
function blockedNoComposer(t: ToolStatus): boolean {
  return t.id === "laravel" && !composerInstalled.value;
}

const uninstallOpen = ref(false);
const uninstallTarget = ref<ToolStatus | null>(null);

// Streamed install log.
const logOpen = ref(false);
const logTool = ref<ToolStatus | null>(null);
const installLog = ref<string[]>([]);
const logBox = ref<HTMLElement | null>(null);

async function appendLog(lines: string[]): Promise<void> {
  installLog.value.push(...lines);
  await nextTick();
  const el = logBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

async function load(): Promise<void> {
  loading.value = true;
  try {
    tools.value = await listTools();
  } catch (e) {
    toast.error("Couldn't load tools", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

async function doInstall(t: ToolStatus): Promise<void> {
  busy.value = `install:${t.id}`;
  const verb = t.installed ? "Updated" : "Installed";
  logTool.value = t;
  installLog.value = [];
  logOpen.value = true;
  try {
    const jobId = await installToolStreamed(t.id);
    const final = await pollJobToEnd(
      jobId,
      (lines) => void appendLog(lines),
      () => logOpen.value,
    );
    await Promise.all([load(), refresh()]);
    if (final.state === "succeeded") {
      toast.success(`${verb} ${t.display_name}`);
    } else if (final.state !== "running") {
      // "running" = the log modal was closed early; the install continues detached.
      toast.error(`Install of ${t.display_name} failed`, final.error ?? "install failed");
    }
  } catch (e) {
    toast.error(`Install of ${t.display_name} failed`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

function openUninstall(t: ToolStatus): void {
  uninstallTarget.value = t;
  uninstallOpen.value = true;
}

async function confirmUninstall(close: () => void): Promise<void> {
  const t = uninstallTarget.value;
  if (!t) return;
  busy.value = `uninstall:${t.id}`;
  close();
  try {
    await uninstallTool(t.id);
    toast.success(`Removed ${t.display_name}`);
    await Promise.all([load(), refresh()]);
  } catch (e) {
    toast.error(`Uninstall of ${t.display_name} failed`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

onMounted(load);
onUnmounted(registerViewActions({ refresh: () => void load() }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Tooling"
      subtitle="Install developer tools - bundled, self-contained, and added to your PATH alongside PHP."
    />

    <div class="flex-1 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Developer tools</CardTitle>
          <CardDescription>
            Each tool installs the latest release (LTS for Node) into Yerd's
            data directory and exposes its commands on your PATH.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-12">
            <Spinner class="size-6" />
          </div>

          <table v-else class="w-full text-sm">
            <thead>
              <tr
                class="border-b text-left text-xs uppercase text-muted-foreground"
              >
                <th class="py-2 pr-4 font-medium">Tool</th>
                <th class="py-2 pr-4 font-medium">Status</th>
                <th class="py-2 pl-4 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="t in tools"
                :key="t.id"
                class="border-b last:border-0"
              >
                <td class="py-3 pr-4">
                  <div class="font-medium text-foreground">
                    {{ t.display_name }}
                  </div>
                  <div v-if="TOOL_HINTS[t.id]" class="text-xs text-muted-foreground">
                    {{ TOOL_HINTS[t.id] }}
                  </div>
                  <div class="text-xs text-muted-foreground/70">
                    {{ t.binaries.join(", ") }}
                  </div>
                </td>
                <td class="py-3 pr-4">
                  <Badge v-if="t.installed" variant="secondary">
                    {{ t.version ?? "installed" }}
                  </Badge>
                  <Badge v-else-if="t.external" variant="outline">
                    External
                  </Badge>
                  <span v-else class="text-xs text-muted-foreground">
                    Not installed
                  </span>
                </td>
                <td class="py-3 pl-4">
                  <div class="flex items-center justify-end gap-2">
                    <Spinner
                      v-if="busy?.endsWith(`:${t.id}`)"
                      class="size-4"
                    />
                    <template v-else-if="t.installed">
                      <Button
                        variant="outline"
                        size="sm"
                        :disabled="busy !== null || blockedNoComposer(t)"
                        :title="blockedNoComposer(t) ? COMPOSER_REQUIRED_HINT : ''"
                        @click="doInstall(t)"
                      >
                        <RefreshCw class="mr-1.5 size-3.5" /> Update
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        :disabled="busy !== null"
                        :aria-label="`Uninstall ${t.display_name}`"
                        title="Uninstall"
                        @click="openUninstall(t)"
                      >
                        <Trash2 class="size-3.5" />
                      </Button>
                    </template>
                    <!-- External tools are managed by the user, not Yerd: no actions. -->
                    <template v-else-if="t.external" />
                    <Button
                      v-else
                      size="sm"
                      :disabled="busy !== null || blockedNoComposer(t)"
                      :title="blockedNoComposer(t) ? COMPOSER_REQUIRED_HINT : ''"
                      @click="doInstall(t)"
                    >
                      <Download class="mr-1.5 size-3.5" /> Install
                    </Button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>
    </div>

    <Modal
      v-model:open="logOpen"
      :title="`Installing ${logTool?.display_name ?? 'tool'}`"
      size="lg"
    >
      <pre
        ref="logBox"
        class="h-72 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ installLog.join("\n") || "Starting…" }}</pre>
      <template #footer="{ close }">
        <Spinner v-if="busy?.startsWith('install:')" class="size-4" />
        <Button :disabled="busy?.startsWith('install:')" @click="close">Done</Button>
      </template>
    </Modal>

    <Modal
      v-model:open="uninstallOpen"
      :title="`Uninstall ${uninstallTarget?.display_name ?? 'tool'}?`"
    >
      <p class="text-sm text-muted-foreground">
        This removes {{ uninstallTarget?.display_name }} and its commands
        ({{ uninstallTarget?.binaries.join(", ") }}) from your PATH. You can
        reinstall it any time.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmUninstall(close)">
          Uninstall
        </Button>
      </template>
    </Modal>
  </div>
</template>
