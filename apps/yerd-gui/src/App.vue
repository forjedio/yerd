<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onMounted, onUnmounted } from "vue";
import { useRoute, useRouter } from "vue-router";

import AppShell from "@/components/AppShell.vue";
import DumpsWindowView from "@/views/DumpsWindowView.vue";
import Toaster from "@/components/ui/Toaster.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import { IpcError, startDaemon, status } from "@/ipc/client";

// The auxiliary "dumps" window renders a standalone viewer with no app shell and
// must NOT run the daemon poller or the first-run auto-start (the main window
// owns both). Branch on the window label, not the route (which is racy at first
// paint).
const isDumpsWindow = getCurrentWindow().label === "dumps";

// Start the single shared daemon poller for the app's lifetime.
const { start, stop, refresh } = useDaemon();
const router = useRouter();
const route = useRoute();
const toast = useToast();
let unlistenNav: UnlistenFn | undefined;

// The separate Mails viewer window loads a `standalone` route: it must render
// bare (no sidebar/titlebar) and must NOT spin up a second daemon poller or the
// first-run start flow (the main window owns those).
const standalone = computed(() => route.meta.standalone === true);

// First-load auto-START of the (bundled) daemon. A module-level guard keeps it
// to one run.
let autoStartDone = false;

/** Is the daemon reachable now? (Direct probe, independent of the poller, which
 *  may have a tick in flight.) Mirrors useDaemon: only an *unreachable* socket
 *  counts as down; a typed daemon error still means it's up. */
async function daemonReachable(): Promise<boolean> {
  try {
    await status();
    return true;
  } catch (e) {
    return !(e instanceof IpcError && e.unreachable);
  }
}

// The daemon is bundled inside the app, so first run just *starts* it (no
// download). If it's already reachable (e.g. autostarted, or `cargo run -p
// yerdd`) we leave it alone — starting a second would compete for the socket.
async function maybeAutoStart(): Promise<void> {
  if (autoStartDone) return;
  autoStartDone = true;
  if (await daemonReachable()) return;
  try {
    await startDaemon();
  } catch (e) {
    // Non-fatal: on macOS the daemon may be pending Login-Items approval, or the
    // user can start it from the General tab — both are surfaced there.
    toast.error("Couldn't start the Yerd daemon", (e as IpcError).message);
  } finally {
    await refresh();
  }
}

onMounted(async () => {
  // The dumps window and the standalone Mails viewer share this SPA bundle but
  // must not duplicate the poller, the tray-nav listener, or the start flow —
  // those belong to the main window.
  if (isDumpsWindow || standalone.value) return;
  start(4000);
  // The tray's "go to <page>" items emit `navigate` with a route path (e.g.
  // "/sites") after showing the window; jump the router there.
  unlistenNav = await listen<string>("navigate", (event) => {
    router.push(event.payload);
  });
  maybeAutoStart();
});

onUnmounted(() => {
  if (isDumpsWindow || standalone.value) return;
  stop();
  unlistenNav?.();
});
</script>

<template>
  <!-- The standalone dumps window renders its viewer directly (no SideNav). -->
  <DumpsWindowView v-if="isDumpsWindow" />
  <!-- Standalone routes (the Mails viewer window) render bare — no shell. -->
  <RouterView v-else-if="standalone" />
  <template v-else>
    <AppShell />
  </template>
  <Toaster />
</template>
