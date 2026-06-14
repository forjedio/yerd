<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";

import AppShell from "@/components/AppShell.vue";
import DumpsWindowView from "@/views/DumpsWindowView.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Toaster from "@/components/ui/Toaster.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  daemonInstalled,
  hostPlatform,
  installDaemon,
  IpcError,
  onInstallProgress,
  startDaemon,
  status,
} from "@/ipc/client";

// The auxiliary "dumps" window renders a standalone viewer with no app shell and
// must NOT run the daemon poller or the first-run auto-install (the main window
// owns both). Branch on the window label, not the route (which is racy at first
// paint).
const isDumpsWindow = getCurrentWindow().label === "dumps";

// Start the single shared daemon poller for the app's lifetime.
const { start, stop, refresh } = useDaemon();
const router = useRouter();
const toast = useToast();
let unlistenNav: UnlistenFn | undefined;

// First-load auto-install of yerdd. A module-level guard keeps it to one run.
let autoInstallDone = false;
const installing = ref(false);
const installMessage = ref("");

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

async function maybeAutoInstall(): Promise<void> {
  if (autoInstallDone) return;
  autoInstallDone = true;

  const platform = await hostPlatform().catch(() => "");
  if (platform !== "linux" && platform !== "macos") return; // Windows: skip.

  // Never install if a daemon is already reachable (e.g. one started outside our
  // search paths, like `cargo run -p yerdd`) — that would start a competitor.
  if (await daemonReachable()) return;
  if (await daemonInstalled()) return; // installed but down → Start from General.

  installing.value = true;
  installMessage.value = "Preparing…";
  const unlisten = await onInstallProgress((m) => {
    installMessage.value = m;
  });
  try {
    await installDaemon();
    await startDaemon();
    router.push("/general");
    toast.success("Yerd is ready", "Installed and started the daemon.");
  } catch (e) {
    toast.error("Couldn't install yerdd", (e as IpcError).message);
  } finally {
    unlisten();
    installing.value = false;
    await refresh();
  }
}

onMounted(async () => {
  if (isDumpsWindow) return;
  start(4000);
  // The tray's "go to <page>" items emit `navigate` with a route path (e.g.
  // "/sites") after showing the window; jump the router there.
  unlistenNav = await listen<string>("navigate", (event) => {
    router.push(event.payload);
  });
  maybeAutoInstall();
});

onUnmounted(() => {
  if (isDumpsWindow) return;
  stop();
  unlistenNav?.();
});
</script>

<template>
  <!-- The standalone dumps window renders its viewer directly (no SideNav). -->
  <DumpsWindowView v-if="isDumpsWindow" />
  <template v-else>
    <AppShell />

    <!-- First-run yerdd install overlay. -->
    <div
      v-if="installing"
      class="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-background/95 text-center"
    >
      <Spinner class="size-6" />
      <p class="text-sm font-medium">Installing Yerdd… Please wait</p>
      <p class="text-xs text-muted-foreground">{{ installMessage }}</p>
    </div>
  </template>
  <Toaster />
</template>
