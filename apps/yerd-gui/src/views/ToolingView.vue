<script setup lang="ts">
import { onMounted, ref } from "vue";
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
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { installTool, IpcError, listTools, uninstallTool } from "@/ipc/client";
import type { ToolStatus } from "@/ipc/types";

const toast = useToast();
const { refresh } = useDaemon();

const tools = ref<ToolStatus[]>([]);
const loading = ref(true);
// Which tool has a long-running op in flight, e.g. "install:node".
const busy = ref<string | null>(null);

const uninstallOpen = ref(false);
const uninstallTarget = ref<ToolStatus | null>(null);

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
  try {
    await installTool(t.id);
    toast.success(`${verb} ${t.display_name}`);
    await Promise.all([load(), refresh()]);
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
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Tooling"
      subtitle="Install developer tools — bundled, self-contained, and added to your PATH alongside PHP."
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
                  <div class="text-xs text-muted-foreground">
                    {{ t.binaries.join(", ") }}
                  </div>
                </td>
                <td class="py-3 pr-4">
                  <Badge v-if="t.installed" variant="secondary">
                    {{ t.version ?? "installed" }}
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
                        :disabled="busy !== null"
                        @click="doInstall(t)"
                      >
                        <RefreshCw class="mr-1.5 size-3.5" /> Update
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        :disabled="busy !== null"
                        @click="openUninstall(t)"
                      >
                        <Trash2 class="size-3.5" />
                      </Button>
                    </template>
                    <Button
                      v-else
                      size="sm"
                      :disabled="busy !== null"
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
