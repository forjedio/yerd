import { computed, readonly, ref } from "vue";

import { IpcError, status as fetchStatus } from "@/ipc/client";
import type { StatusReport } from "@/ipc/types";

/**
 * Singleton daemon store.
 *
 * One poller for the whole app (the global connection pill, the tray, and the
 * Services/PHP views all read this), so the daemon isn't hit by N independent
 * `status` loops. `status` doubles as the liveness probe - a successful report
 * means "connected". Started/stopped from App.vue's lifecycle.
 */
const report = ref<StatusReport | null>(null);
const lastError = ref<IpcError | null>(null);
const connected = ref<boolean | null>(null); // null = not yet probed
const polling = ref(false);

let timer: ReturnType<typeof setTimeout> | null = null;
let inFlight = false;
let intervalMs = 4000;

async function tick(): Promise<void> {
  if (inFlight) return;
  if (document.visibilityState === "hidden") {
    schedule();
    return;
  }
  inFlight = true;
  try {
    report.value = await fetchStatus();
    connected.value = true;
    lastError.value = null;
  } catch (e) {
    const err = e instanceof IpcError ? e : new IpcError(String(e));
    lastError.value = err;
    // Only a genuine unreachable socket flips us to "disconnected"; a typed
    // daemon error still means the daemon is up.
    connected.value = !err.unreachable;
    // When the socket is truly gone, drop the last report so no view keeps
    // rendering stale "running" rows (pid/uptime/memory) under a "Stopped"
    // header after the daemon dies mid-session.
    if (err.unreachable) report.value = null;
  } finally {
    inFlight = false;
    schedule();
  }
}

function schedule(): void {
  if (!polling.value) return;
  if (timer) clearTimeout(timer);
  timer = setTimeout(tick, intervalMs);
}

function start(ms = 4000): void {
  intervalMs = ms;
  if (polling.value) return;
  polling.value = true;
  void tick();
}

function stop(): void {
  polling.value = false;
  if (timer) clearTimeout(timer);
  timer = null;
}

async function refresh(): Promise<void> {
  if (timer) clearTimeout(timer);
  await tick();
}

export function useDaemon() {
  return {
    // Raw ref (not readonly-wrapped) so views can pass the report to functions
    // typed `StatusReport` without DeepReadonly friction. By convention only
    // this store mutates it.
    report,
    lastError: readonly(lastError),
    connected: readonly(connected),
    /** True only when we've probed and the socket was unreachable. */
    unreachable: computed(() => connected.value === false),
    start,
    stop,
    refresh,
  };
}
