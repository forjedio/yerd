<script setup lang="ts">
import { PlugZap, Play, RefreshCw } from "lucide-vue-next";
import { computed, ref } from "vue";
import { useRoute } from "vue-router";

import SideNav from "@/components/SideNav.vue";
import TitleBar from "@/components/TitleBar.vue";
import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { IpcError, startDaemon } from "@/ipc/client";

const route = useRoute();
const { unreachable, refresh } = useDaemon();
const toast = useToast();
const starting = ref(false);

// Some routes don't need a live daemon: General can start/install it, Services
// is a coming-soon placeholder, and About degrades gracefully. Only the
// daemon-dependent views (PHP, Sites, Doctor) are blocked by the not-running panel.
const DAEMON_FREE = new Set(["general", "services", "about"]);
const showPanel = computed(
  () => unreachable.value && !DAEMON_FREE.has(String(route.name)),
);

async function onStart(): Promise<void> {
  starting.value = true;
  try {
    await startDaemon();
    toast.success("Starting daemon…", "It should connect in a moment.");
  } catch (e) {
    toast.error("Couldn't start the daemon", (e as IpcError).message);
  } finally {
    starting.value = false;
    await refresh();
  }
}
</script>

<template>
  <div class="flex h-full w-full flex-col overflow-hidden">
    <TitleBar />

    <div class="flex min-h-0 flex-1 overflow-hidden">
      <SideNav />

      <main class="flex-1 overflow-y-auto">
        <!-- Daemon-dependent routes show this when the socket is unreachable.
             The General tab is exempt — it can start/install the daemon. -->
        <div
          v-if="showPanel"
          class="flex h-full flex-col items-center justify-center gap-4 p-8 text-center"
        >
          <PlugZap class="size-10 text-muted-foreground" />
          <div>
            <h2 class="text-lg font-semibold">Daemon not running</h2>
            <p class="mt-1 max-w-sm text-sm text-muted-foreground">
              Yerd can't reach the <code>yerdd</code> daemon socket. Start it
              below, or manage it from the <strong>General</strong> tab.
            </p>
          </div>
          <div class="flex gap-2">
            <Button :disabled="starting" @click="onStart">
              <Spinner v-if="starting" class="size-4" />
              <Play v-else class="size-4" /> Start daemon
            </Button>
            <Button variant="outline" @click="refresh">
              <RefreshCw class="size-4" /> Retry
            </Button>
          </div>
        </div>

        <RouterView v-else />
      </main>
    </div>
  </div>
</template>
