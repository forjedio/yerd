<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onMounted, onUnmounted } from "vue";
import { useRoute, useRouter } from "vue-router";

import AppShell from "@/components/AppShell.vue";
import DumpsWindowView from "@/views/DumpsWindowView.vue";
import MailsViewerView from "@/views/MailsViewerView.vue";
import WelcomeView from "@/views/WelcomeView.vue";
import Toaster from "@/components/ui/Toaster.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useOnboarding } from "@/composables/useOnboarding";
import { useToast } from "@/composables/useToast";
import { useShortcuts } from "@/lib/shortcuts/useShortcuts";
import { sitesIntent } from "@/lib/shortcuts/sitesIntent";
import { IpcError, startDaemon } from "@/ipc/client";

// The auxiliary "dumps" and "mails" windows render standalone viewers with no app
// shell and must NOT run the daemon poller or the first-run auto-start (the main
// window owns both). Branch on the window label, not the route (which is racy at
// first paint: Vue Router's initial navigation is async, so `route.meta` is still
// empty on first render and a route-based check would briefly fall through to the
// main AppShell).
const windowLabel = getCurrentWindow().label;
const isDumpsWindow = windowLabel === "dumps";
const isMailsWindow = windowLabel === "mails";

if (isDumpsWindow) useShortcuts("dumps");
else if (isMailsWindow) useShortcuts("mails");

// Start the single shared daemon poller for the app's lifetime.
const { start, stop, refresh } = useDaemon();
const { probing, needsOnboarding, probe } = useOnboarding();
const router = useRouter();
const route = useRoute();
const toast = useToast();
let unlistenNav: UnlistenFn | undefined;
let unlistenSitesIntent: UnlistenFn | undefined;

// The separate Mails viewer window loads a `standalone` route: it must render
// bare (no sidebar/titlebar) and must NOT spin up a second daemon poller or the
// first-run start flow (the main window owns those).
const standalone = computed(() => route.meta.standalone === true);

// The daemon is bundled inside the app, so first run just *starts* it (no
// download). If it's already reachable (e.g. autostarted, or `cargo run -p
// yerdd`) we leave it alone - starting a second would compete for the socket.
async function autoStart(): Promise<void> {
  try {
    await startDaemon();
  } catch (e) {
    // Non-fatal: on macOS the daemon may be pending Login-Items approval, or the
    // user can start it from the General tab - both are surfaced there.
    toast.error("Couldn't start the Yerd daemon", (e as IpcError).message);
  } finally {
    await refresh();
  }
}

onMounted(async () => {
  // The dumps window and the standalone Mails viewer share this SPA bundle but
  // must not duplicate the poller, the tray-nav listener, or the start flow -
  // those belong to the main window.
  if (isDumpsWindow || isMailsWindow || standalone.value) return;
  start(4000);
  // Run the probe FIRST and clear the splash before anything that can throw - a
  // failing `listen` below must never strand the user on the splash forever. One
  // shared probe decides between the first-run journey, the start screen, and
  // auto-starting the bundled daemon; the journey owns starting the daemon, so
  // skip auto-start when it's showing.
  const { reachable } = await probe();
  // The tray's "go to <page>" items emit `navigate` with a route path (e.g.
  // "/sites") after showing the window; jump the router there. Best-effort.
  try {
    unlistenNav = await listen<string>("navigate", (event) => {
      router.push(event.payload);
    });
    // The tray's "New Laravel site…" emits `sites-intent` ("create"); set the
    // intent then route to /sites, where SitesView opens the matching dialog.
    // Validate the payload (external boundary) before trusting the union cast.
    unlistenSitesIntent = await listen<string>("sites-intent", (event) => {
      if (event.payload !== "link" && event.payload !== "park" && event.payload !== "create") {
        return;
      }
      sitesIntent.value = event.payload;
      router.push("/sites");
    });
  } catch {
    /* tray navigation is non-critical */
  }
  if (needsOnboarding.value) return;
  if (!reachable) await autoStart();
});

onUnmounted(() => {
  if (isDumpsWindow || isMailsWindow || standalone.value) return;
  stop();
  unlistenNav?.();
  unlistenSitesIntent?.();
});
</script>

<template>
  <!-- The standalone dumps window renders its viewer directly (no SideNav). -->
  <DumpsWindowView v-if="isDumpsWindow" />
  <!-- The standalone mails window renders its viewer directly (no SideNav).
       Branch on the window label, not the route, to avoid the first-paint race. -->
  <MailsViewerView v-else-if="isMailsWindow" />
  <!-- Other standalone routes render bare - no shell. -->
  <RouterView v-else-if="standalone" />
  <!-- First-run probe in flight: a brief splash so we never flash the wrong
       screen (the daemon-stopped panel) before the journey/app is decided. -->
  <div
    v-else-if="probing"
    class="flex h-full w-full items-center justify-center bg-background"
  >
    <Spinner class="size-5 text-muted-foreground" />
  </div>
  <!-- Never set up → the full-screen welcome journey. -->
  <WelcomeView v-else-if="needsOnboarding" />
  <template v-else>
    <AppShell />
  </template>
  <Toaster />
</template>
