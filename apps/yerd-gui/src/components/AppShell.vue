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

// Only three views work without a live daemon: Overview (owns its own
// daemon-down hero + start affordance), Settings/General (can start/install it),
// and About (pure build info). Every data-backed view — PHP, Sites, Tooling,
// Services, Dumps, Mail, Doctor — is blocked by the not-running panel so none of
// them mount, fire a doomed request, and strand the user on a blank table.
const DAEMON_FREE = new Set(["overview", "general", "about"]);
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

      <!-- Clip, don't scroll: every routed view is `h-full` and owns its own
           inner `overflow-y-auto`, so a scroll container here would be a second,
           redundant scrollbar nested inside the view's own. -->
      <main class="min-w-0 flex-1 overflow-hidden">
        <!-- Daemon-dependent routes show this when the socket is unreachable.
             Overview / Settings / About are exempt (see DAEMON_FREE) - they can
             start/install the daemon or degrade gracefully. -->
        <div
          v-if="showPanel"
          class="flex h-full flex-col items-center justify-center gap-4 p-8 text-center"
        >
          <PlugZap class="size-10 text-muted-foreground" />
          <div>
            <h2 class="text-lg font-semibold">Daemon not running</h2>
            <p class="mt-1 max-w-sm text-sm text-muted-foreground">
              Yerd can't reach the <code>yerdd</code> daemon socket. Start it
              below to use this page.
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
