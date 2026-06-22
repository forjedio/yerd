<script setup lang="ts">
import { getVersion } from "@tauri-apps/api/app";
import { Info } from "lucide-vue-next";
import { onMounted, ref } from "vue";

import logoUrl from "@/assets/logo.svg";
import PageHeader from "@/components/PageHeader.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, openInBrowser, protocolVersion, status } from "@/ipc/client";

const toast = useToast();

const appVersion = ref("");
const protocol = ref<number | null>(null);
const daemonVersion = ref("");

onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch {
    /* not in a Tauri context (e.g. tests) — leave blank */
  }
  try {
    const [p, report] = await Promise.all([protocolVersion(), status()]);
    protocol.value = p;
    daemonVersion.value = report.daemon_version;
  } catch (e) {
    // The page degrades gracefully (daemon fields read "unknown"/"—"), so a
    // down daemon needs no alarm here — only surface a real, unexpected error.
    const err = e as IpcError;
    if (!err.unreachable) toast.error("Couldn't load daemon info", err.message);
  }
});
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="About" subtitle="Build info and links" />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <!-- Identity + links -->
      <Card>
        <CardContent class="flex flex-col items-center gap-3 py-8 text-center">
          <img :src="logoUrl" alt="Yerd" class="size-20" />
          <div class="space-y-1">
            <p class="text-lg font-semibold">Yerd</p>
            <p class="text-xs text-muted-foreground">
              A cross-platform local PHP development environment.
            </p>
          </div>
          <div class="flex flex-wrap items-center justify-center gap-x-4 gap-y-1 text-sm">
            <button class="text-brand hover:underline" @click="openInBrowser('https://yerd.app')">
              yerd.app
            </button>
            <button
              class="text-brand hover:underline"
              @click="openInBrowser('https://github.com/forjedio/yerd')"
            >
              GitHub
            </button>
            <button class="text-brand hover:underline" @click="openInBrowser('https://forjed.io')">
              forjed.io
            </button>
          </div>
          <p class="text-xs text-muted-foreground">Licensed under MIT.</p>
        </CardContent>
      </Card>

      <!-- Versions -->
      <Card>
        <CardHeader><CardTitle class="flex items-center gap-2"><Info class="size-4" /> Versions</CardTitle></CardHeader>
        <CardContent class="space-y-2 text-sm">
          <div class="flex justify-between">
            <span class="text-muted-foreground">App version</span>
            <span class="font-mono">{{ appVersion || "-" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Daemon version</span>
            <span class="font-mono">{{ daemonVersion || "unknown" }}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">IPC protocol</span>
            <span class="font-mono">{{ protocol ?? "-" }}</span>
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</template>
