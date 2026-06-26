<script setup lang="ts">
import { ExternalLink, Play } from "lucide-vue-next";
import { computed } from "vue";

import DaemonDiagnosticsPanel from "@/components/DaemonDiagnosticsPanel.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useDaemonStart } from "@/composables/useDaemonStart";
import { openLoginItems } from "@/ipc/client";
import logoUrl from "@/assets/logo.svg";

// The shared "Yerd isn't running" hero + start affordance. Used by the Overview
// page and as the blocked-page screen in AppShell, so every page shows the same
// thing when the daemon is down. The start→wait→diagnose logic (with auto-clear
// on connect) lives in useDaemonStart; on failure we show actionable diagnostics
// instead of a blind toast, and on macOS pending-approval we show the Login-Items
// affordance rather than a "failure".
const { report } = useDaemon();
const { starting, pendingApproval, diagnostics, start } = useDaemonStart();
const tld = computed(() => report.value?.tld ?? "test");

function onStart(): void {
  // nudge=true: a single-button start may open Login Items on pending approval.
  void start({ nudge: true });
}
</script>

<template>
  <Card class="flex flex-col items-center justify-center gap-4 py-16 text-center">
    <img :src="logoUrl" alt="" class="size-12 rounded-xl" />
    <div>
      <h2 class="text-lg font-semibold">Yerd isn't running</h2>
      <p class="mx-auto mt-1 max-w-sm text-sm text-muted-foreground">
        Start the daemon to serve your <span class="font-mono">.{{ tld }}</span>
        sites, PHP runtimes, and services.
      </p>
    </div>

    <!-- macOS: registered but awaiting the user's Login-Items approval. -->
    <div
      v-if="pendingApproval"
      class="mx-auto max-w-md rounded-md border border-warning/40 bg-warning/10 p-3 text-left text-sm"
    >
      <p class="font-medium">One more step</p>
      <p class="mt-1 text-muted-foreground">
        macOS needs you to allow Yerd in the background. Enable it under Login
        Items, then it'll connect automatically.
      </p>
      <Button variant="outline" size="sm" class="mt-2" @click="openLoginItems">
        <ExternalLink class="size-4" /> Open Login Items
      </Button>
    </div>

    <!-- Why the daemon didn't come up. -->
    <DaemonDiagnosticsPanel
      v-else-if="diagnostics"
      :diagnostics="diagnostics"
      class="mx-auto max-w-md text-left"
    />

    <Button :disabled="starting" @click="onStart">
      <Spinner v-if="starting" class="size-4" />
      <Play v-else class="size-4" /> Start Yerd
    </Button>
  </Card>
</template>
