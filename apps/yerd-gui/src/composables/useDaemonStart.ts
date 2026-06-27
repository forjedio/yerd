import { computed, onMounted, onUnmounted, ref } from "vue";
import { watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { useDaemon } from "@/composables/useDaemon";
import { daemonDiagnostics, IpcError, startDaemon } from "@/ipc/client";
import type { DaemonDiagnostics } from "@/ipc/types";
import { log } from "@/lib/log";

/**
 * Shared "start the daemon, wait for it to actually connect, and diagnose on
 * failure" skeleton - used by the onboarding step 1 and the DaemonDownHero so
 * both surface the same diagnostics instead of a blind 20 s toast.
 *
 * Deliberately narrow: it owns only the phase / timeout / fast-poll / diagnose
 * state. It does NOT own macOS login-item orchestration - that ordering is
 * load-bearing in onboarding (enable the GUI login item, *then* re-probe
 * approval) and must NOT run from the hero's "Start Yerd" button. Callers inject
 * that via the `beforeProbe` hook, which runs after `startDaemon` and returns
 * whether the daemon/GUI is now pending Login-Items approval.
 *
 * Phased button: "starting the daemon" may install / upgrade / start it before
 * the readiness wait. The Rust host emits a `daemon-start-phase` event as it
 * walks those steps (each with its own per-phase timeout); we reflect it in
 * `phase`, which drives a step label on the button ("Installing Daemon" →
 * "Starting Daemon" → "Running Daemon"), resetting to idle on completion. The
 * frontend owns the `running` (readiness wait) and `idle` phases; Rust owns the
 * earlier ones.
 *
 * Per-instance state (declared here, in component setup scope) + a
 * component-scoped `watch`/`onMounted`/`onUnmounted`, so the two mount sites
 * don't share a timer OR a phase listener. `connected` only updates when the
 * shared `useDaemon` poller ticks, so the fast-poll actively drives `refresh()`.
 */

export interface StartOptions {
  /** macOS: open Login Items on a pending approval. Onboarding passes false. */
  nudge?: boolean;
  /**
   * Runs after `startDaemon` resolves; returns whether the daemon/GUI is pending
   * Login-Items approval. Onboarding injects its enable-login-defaults +
   * re-probe here; the hero omits it (and so never enables login-at-boot).
   */
  beforeProbe?: () => Promise<boolean>;
}

/** Idle, the three Rust-driven service-manager phases, then the readiness wait. */
export type StartPhase = "idle" | "installing" | "upgrading" | "starting" | "running";

const POLL_MS = 500;
// The readiness ("running") wait gets its own ceiling, separate from the
// per-phase budgets the Rust host enforces on install/upgrade/start. Sized to
// outlast a cold daemon start after an upgrade (the surviving instance still
// boots and warms its services) so a normal upgrade never trips a false timeout.
const RUNNING_CEILING_MS = 30_000;

/** Button label for a phase, or `null` when idle (callers fall back to their
 * own resting label). */
function labelFor(p: StartPhase): string | null {
  switch (p) {
    case "installing":
      return "Installing Daemon";
    case "upgrading":
      return "Upgrading Daemon";
    case "starting":
      return "Starting Daemon";
    case "running":
      return "Running Daemon";
    default:
      return null;
  }
}

export function useDaemonStart() {
  const { connected, refresh } = useDaemon();

  // `phase` is the single source of truth; `starting` is derived so the existing
  // `:disabled` / spinner bindings keep working unchanged.
  const phase = ref<StartPhase>("idle");
  const starting = computed(() => phase.value !== "idle");
  const activeLabel = computed(() => labelFor(phase.value));
  const pendingApproval = ref(false);
  const diagnostics = ref<DaemonDiagnostics | null>(null);

  let pollTimer: ReturnType<typeof setTimeout> | undefined;
  let elapsed = 0;
  // Set once the component unmounts so an in-flight `await` (refresh/diagnose)
  // doesn't resume and reschedule a timer / mutate a dead component.
  let disposed = false;
  // True only from `start()` entry until `startDaemon` resolves - the window in
  // which the Rust host owns the phase label. Per-instance, so a non-initiating
  // mounted instance ignores the global phase event.
  let acceptRustPhases = false;
  let unlistenPhase: UnlistenFn | undefined;

  function clearPoll(): void {
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = undefined;
    }
  }

  function reset(): void {
    clearPoll();
    acceptRustPhases = false;
    phase.value = "idle";
    pendingApproval.value = false;
    diagnostics.value = null;
  }

  /** True once the daemon connected or the component went away - never show a
   * failure panel in that case (avoids "Running" + failure panel together). */
  function stale(): boolean {
    return disposed || connected.value === true;
  }

  async function diagnose(startError?: string): Promise<void> {
    try {
      const d = await daemonDiagnostics(startError);
      if (stale()) return; // connected/unmounted while gathering - don't contradict
      // A registered-but-unapproved daemon isn't a failure: show the approval
      // affordance, not the diagnostics panel.
      if (d.pendingApproval) {
        pendingApproval.value = true;
        diagnostics.value = null;
      } else {
        diagnostics.value = d;
      }
    } catch {
      if (stale()) return;
      // Diagnostics gathering itself failed - still surface something actionable
      // rather than a silent dead-end.
      diagnostics.value = minimalDiagnostics(
        startError ?? "The daemon didn't come up, and diagnostics couldn't be gathered.",
      );
    }
  }

  function beginPolling(startError?: string): void {
    clearPoll();
    elapsed = 0;
    phase.value = "running"; // the readiness wait - frontend-owned
    const tick = async (): Promise<void> => {
      if (disposed || !starting.value) return;
      await refresh(); // drives the shared poller; updates `connected`
      if (disposed || !starting.value) return; // re-check after the await
      if (connected.value === true) return; // the watch below clears state
      elapsed += POLL_MS;
      if (elapsed >= RUNNING_CEILING_MS) {
        log.warn("daemon start: readiness wait timed out");
        await diagnose(startError);
        phase.value = "idle";
        return;
      }
      pollTimer = setTimeout(() => void tick(), POLL_MS);
    };
    pollTimer = setTimeout(() => void tick(), POLL_MS);
  }

  async function start(opts: StartOptions = {}): Promise<void> {
    reset();
    // Show the spinner immediately on click: the macOS plan can spend a few
    // seconds probing launchctl before the first phase event arrives.
    phase.value = "starting";
    acceptRustPhases = true;
    log.info("daemon start requested");
    let startError: string | undefined;
    try {
      await startDaemon(opts.nudge ?? false);
    } catch (e) {
      // Launch threw outright (missing sidecar, translocation refusal, register
      // failure) - capture the message and diagnose immediately.
      acceptRustPhases = false;
      startError = (e as IpcError).message;
      log.error(`daemon start failed: ${startError}`);
      await diagnose(startError);
      phase.value = "idle";
      return;
    }
    // Rust is done walking its steps; the frontend now owns running/idle so a
    // straggler phase event can't flip the label back.
    acceptRustPhases = false;
    // Let the caller enable login items / re-probe approval, then read it.
    if (opts.beforeProbe) {
      try {
        pendingApproval.value = await opts.beforeProbe();
      } catch {
        /* best-effort; treat as not-pending */
      }
    }
    await refresh();
    if (connected.value === true) {
      phase.value = "idle";
      return;
    }
    if (pendingApproval.value) {
      // Can't connect until the user approves; show the affordance, not a panel.
      phase.value = "idle";
      return;
    }
    beginPolling(startError);
  }

  // Once the daemon is actually reachable, drop any failure/approval UI.
  watch(connected, (c) => {
    if (c === true) {
      acceptRustPhases = false;
      phase.value = "idle";
      pendingApproval.value = false;
      diagnostics.value = null;
      clearPoll();
      log.debug("daemon connected");
    }
  });

  // One always-on phase listener per instance; gated by `acceptRustPhases` so
  // only the instance whose `start()` is in flight reflects the event.
  onMounted(async () => {
    try {
      const un = await listen<string>("daemon-start-phase", (e) => {
        if (!acceptRustPhases || disposed) return;
        const p = e.payload;
        if (p === "installing" || p === "upgrading" || p === "starting") {
          phase.value = p;
          log.debug(`daemon start phase: ${p}`);
        }
      });
      // If we unmounted while `listen` was awaiting, onUnmounted already ran (and
      // saw no handle) - drop this now-orphaned registration instead of leaking it.
      if (disposed) un();
      else unlistenPhase = un;
    } catch {
      /* events unavailable (non-Tauri/test context) - phases just won't update */
    }
  });

  onUnmounted(() => {
    disposed = true;
    clearPoll();
    if (unlistenPhase) {
      unlistenPhase();
      unlistenPhase = undefined;
    }
  });

  return { starting, phase, activeLabel, pendingApproval, diagnostics, start, reset };
}

/** Minimal fallback when `daemon_diagnostics` itself can't run - the message
 * doubles as the single actionable hint so the panel is never empty. */
function minimalDiagnostics(message: string): DaemonDiagnostics {
  return {
    startError: message,
    hints: [message],
    yerddPath: null,
    translocated: false,
    socketPath: "",
    socketResponding: false,
    lastConnectError: null,
    serviceManager: "",
    serviceStatus: null,
    pendingApproval: false,
    logPath: null,
    logTail: [],
    spawnLogTail: [],
    repairLogTail: [],
  };
}
