<script setup lang="ts">
import { onMounted, ref } from "vue";
import { getVersion } from "@tauri-apps/api/app";
import { Copy, FolderOpen, Info, Link, Network } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import {
  daemonInfo,
  IpcError,
  openInBrowser,
  openPath,
  protocolVersion,
  status,
} from "@/ipc/client";
import type { InfoResponse } from "@/ipc/types";

const toast = useToast();

const info = ref<InfoResponse | null>(null);
const appVersion = ref("");
const protocol = ref<number | null>(null);
const daemonVersion = ref("");
const loading = ref(true);

async function copy(text: string, what: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast.info(`Copied ${what}`);
  } catch {
    toast.error("Couldn't copy");
  }
}

onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch {
    /* not in a Tauri context (e.g. tests) — leave blank */
  }
  try {
    const [i, p, report] = await Promise.all([daemonInfo(), protocolVersion(), status()]);
    info.value = i;
    protocol.value = p;
    daemonVersion.value = report.daemon_version;
  } catch (e) {
    toast.error("Couldn't load daemon info", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="About" subtitle="Yerd build and local CA details" />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <Card>
        <CardHeader><CardTitle class="flex items-center gap-2"><Info class="size-4" /> Yerd</CardTitle></CardHeader>
        <CardContent class="space-y-2 text-sm">
          <div class="flex justify-between">
            <span class="text-muted-foreground">App version</span>
            <span class="font-mono">{{ appVersion || "—" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Daemon version</span>
            <span class="font-mono">{{ daemonVersion || "unknown" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">IPC protocol</span>
            <span class="font-mono">{{ protocol ?? "—" }}</span>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader><CardTitle class="flex items-center gap-2"><Network class="size-4" /> Local environment</CardTitle></CardHeader>
        <CardContent>
          <div v-if="loading" class="flex justify-center py-8"><Spinner class="size-5" /></div>
          <div v-else-if="info" class="space-y-3 text-sm">
            <div class="flex items-center justify-between gap-4">
              <span class="text-muted-foreground">TLD</span>
              <span class="font-mono">.{{ info.tld }}</span>
            </div>
            <div class="flex items-center justify-between gap-4">
              <span class="text-muted-foreground">DNS responder</span>
              <span class="font-mono">{{ info.dns_addr }}</span>
            </div>
            <div class="flex items-center justify-between gap-4">
              <span class="shrink-0 text-muted-foreground">CA certificate</span>
              <span class="flex min-w-0 items-center gap-1">
                <span class="truncate font-mono text-xs" :title="info.ca_path">{{ info.ca_path }}</span>
                <Button variant="ghost" size="icon" title="Reveal" @click="openPath(info.ca_path)">
                  <FolderOpen class="size-4" />
                </Button>
                <Button variant="ghost" size="icon" title="Copy path" @click="copy(info.ca_path, 'CA path')">
                  <Copy class="size-4" />
                </Button>
              </span>
            </div>
            <div class="flex items-center justify-between gap-4">
              <span class="shrink-0 text-muted-foreground">CA fingerprint</span>
              <span class="flex min-w-0 items-center gap-1">
                <span class="truncate font-mono text-xs" :title="info.ca_fingerprint">{{ info.ca_fingerprint }}</span>
                <Button
                  variant="ghost"
                  size="icon"
                  title="Copy fingerprint"
                  @click="copy(info.ca_fingerprint, 'fingerprint')"
                >
                  <Copy class="size-4" />
                </Button>
              </span>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader><CardTitle class="flex items-center gap-2"><Link class="size-4" /> Links</CardTitle></CardHeader>
        <CardContent class="space-y-2 text-sm">
          <button
            class="block text-primary hover:underline"
            @click="openInBrowser('https://github.com/LumoSolutions/yerd')"
          >
            Project repository
          </button>
          <p class="text-xs text-muted-foreground">Licensed under MIT OR Apache-2.0.</p>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
