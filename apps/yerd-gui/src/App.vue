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
import { useDaemonStart } from "@/composables/useDaemonStart";
import { useOnboarding } from "@/composables/useOnboarding";
import { useShortcuts } from "@/lib/shortcuts/useShortcuts";
import { sitesIntent } from "@/lib/shortcuts/sitesIntent";

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
const { start, stop } = useDaemon();
const { start: startDaemonFlow } = useDaemonStart();
const { probing, needsOnboarding, probe } = useOnboarding();
const router = useRouter();
const route = useRoute();
let unlistenNav: UnlistenFn | undefined;
let unlistenSitesIntent: UnlistenFn | undefined;

// The separate Mails viewer window loads a `standalone` route: it must render
// bare (no sidebar/titlebar) and must NOT spin up a second daemon poller or the
// first-run start flow (the main window owns those).
const standalone = computed(() => route.meta.standalone === true);

// The daemon is bundled inside the app, so first run just *starts* it (no
// download). If it's already reachable (e.g. autostarted, or `cargo run -p
// yerdd`) we leave it alone - starting a second would compete for the socket.
// Routed through the shared start->poll->diagnose flow (the same one behind the
// "Start Yerd" button) instead of a raw IPC call, so DaemonDownHero and the
// SideNav operations indicator reflect "starting" immediately instead of
// showing a static "not running" screen until a manual click retries it.
async function autoStart(): Promise<void> {
  await startDaemonFlow();
}

/**
 * Subscribe to the tray's navigation events for the main window. `navigate`
 * carries a route path the tray's "go to <page>" items emit after showing the
 * window; `sites-intent` carries a "link"/"park"/"create" action from the
 * New-site / Link / Park items (validated at this external boundary), which routes
 * to /sites where SitesView opens the matching dialog. Registered before the probe
 * so an event fired during the probe round-trip isn't dropped, and wrapped so a
 * `listen` failure (tray nav is non-critical) never strands the user on the splash.
 */
async function registerTrayNav(): Promise<void> {
  try {
    unlistenNav = await listen<string>("navigate", (event) => {
      router.push(event.payload);
    });
    unlistenSitesIntent = await listen<string>("sites-intent", (event) => {
      if (event.payload !== "link" && event.payload !== "park" && event.payload !== "create") {
        return;
      }
      sitesIntent.value = event.payload;
      router.push("/sites");
    });
  } catch {
    unlistenNav?.();
    unlistenSitesIntent?.();
    unlistenNav = undefined;
    unlistenSitesIntent = undefined;
  }
}

onMounted(async () => {
  if (isDumpsWindow || isMailsWindow || standalone.value) return;
  start(4000);
  await registerTrayNav();
  const { reachable } = await probe();
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
