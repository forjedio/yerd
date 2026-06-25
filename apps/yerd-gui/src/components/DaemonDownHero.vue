<script setup lang="ts">
import { Play } from "lucide-vue-next";
import { computed, onUnmounted, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { IpcError, startDaemon } from "@/ipc/client";
import logoUrl from "@/assets/logo.svg";

// The shared "Yerd isn't running" hero + start affordance. Used by the Overview
// page and as the blocked-page screen in AppShell, so every page shows the same
// thing when the daemon is down. Keeps the button spinning until the daemon
// actually connects — with a timeout so it can never stick forever.
const { connected, report, refresh } = useDaemon();
const toast = useToast();
const starting = ref(false);
const tld = computed(() => report.value?.tld ?? "test");

// If a started daemon never connects (e.g. crashed, or macOS Login-Items
// approval pending), stop spinning after this so the button is clickable again.
const START_TIMEOUT_MS = 20_000;
let startTimer: ReturnType<typeof setTimeout> | undefined;
function clearStartTimer(): void {
  if (startTimer) {
    clearTimeout(startTimer);
    startTimer = undefined;
  }
}

async function onStart(): Promise<void> {
  starting.value = true;
  try {
    await startDaemon();
    await refresh();
    clearStartTimer();
    startTimer = setTimeout(() => {
      if (starting.value && connected.value !== true) {
        starting.value = false;
        toast.error(
          "The daemon didn't come up",
          "Try again, or check Settings → Login Items.",
        );
      }
    }, START_TIMEOUT_MS);
  } catch (e) {
    starting.value = false;
    toast.error("Couldn't start the daemon", (e as IpcError).message);
  }
}

// Stop the spinner once the daemon is reachable.
watch(connected, (c) => {
  if (c === true) {
    starting.value = false;
    clearStartTimer();
  }
});

onUnmounted(clearStartTimer);
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
    <Button :disabled="starting" @click="onStart">
      <Spinner v-if="starting" class="size-4" />
      <Play v-else class="size-4" /> Start Yerd
    </Button>
  </Card>
</template>
