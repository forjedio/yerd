<script setup lang="ts">
import { ExternalLink, Play, ShieldCheck } from "lucide-vue-next";
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
// affordance rather than a "failure". An optional default slot renders a quiet
// note at the bottom of the card (the Overview's footer line).
const { report } = useDaemon();
const { starting, activeLabel, pendingApproval, diagnostics, start } = useDaemonStart();
const tld = computed(() => report.value?.tld ?? "test");
const startLabel = computed(
  () => activeLabel.value ?? (pendingApproval.value ? "Waiting for approval…" : "Start Yerd"),
);

function onStart(): void {
  // nudge=true: a single-button start may open Login Items on pending approval.
  void start({ nudge: true });
}

// `openLoginItems` is an async IPC call; swallow any rejection so a raw `@click`
// binding can't surface an unhandled promise rejection (it's best-effort UX).
function onOpenLoginItems(): void {
  void openLoginItems().catch(() => {});
}
</script>

<template>
  <Card class="flex h-full flex-col items-center rounded-xl px-8 py-12 text-center">
    <div class="flex w-full flex-1 flex-col items-center justify-center">
      <img :src="logoUrl" alt="" class="size-14 rounded-[14px]" />
      <h2 class="mt-4 text-xl font-semibold tracking-[-0.2px]">Yerd isn't running</h2>
      <p class="mx-auto mt-1.5 max-w-sm text-[13px] text-muted-foreground">
        Start the daemon to serve your <span class="font-mono">.{{ tld }}</span>
        sites, PHP and services.
      </p>

      <div
        v-if="pendingApproval"
        data-testid="login-items-panel"
        class="mt-6 w-full max-w-[468px] rounded-[10px] border border-warning/50 bg-warning/5 p-4 text-left text-[13px]"
      >
        <div class="flex items-center gap-2">
          <ShieldCheck class="size-4 shrink-0 text-warning" aria-hidden="true" />
          <p class="font-semibold">One more step</p>
        </div>
        <p class="mt-2 text-muted-foreground">
          macOS needs you to allow Yerd in the background. Turn it on under Login
          Items and Yerd connects automatically.
        </p>
        <Button variant="outline" size="sm" class="mt-3" @click="onOpenLoginItems">
          <ExternalLink /> Open Login Items
        </Button>
      </div>

      <Button class="mt-6" :disabled="starting" @click="onStart">
        <Spinner v-if="starting || pendingApproval" class="size-4 text-current" />
        <Play v-else /> {{ startLabel }}
      </Button>

      <DaemonDiagnosticsPanel
        v-if="!pendingApproval && diagnostics"
        :diagnostics="diagnostics"
        class="mt-5 w-full max-w-[468px] text-left"
      />
    </div>

    <div v-if="$slots.default" class="mt-auto pt-6 text-xs text-faint">
      <slot />
    </div>
  </Card>
</template>
