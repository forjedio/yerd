import { onUnmounted, ref } from "vue";
import { watch } from "vue";

import { useDaemon } from "@/composables/useDaemon";
import { daemonDiagnostics, IpcError, startDaemon } from "@/ipc/client";
import type { DaemonDiagnostics } from "@/ipc/types";

/**
 * Shared "start the daemon, wait for it to actually connect, and diagnose on
 * failure" skeleton — used by the onboarding step 1 and the DaemonDownHero so
 * both surface the same diagnostics instead of a blind 20 s toast.
 *
 * Deliberately narrow: it owns only the timeout / fast-poll / diagnose state.
 * It does NOT own macOS login-item orchestration — that ordering is load-bearing
 * in onboarding (enable the GUI login item, *then* re-probe approval) and must
 * NOT run from the hero's "Start Yerd" button. Callers inject that via the
 * `beforeProbe` hook, which runs after `startDaemon` and returns whether the
 * daemon/GUI is now pending Login-Items approval.
 *
 * Per-instance state (declared here, in component setup scope) + a
 * component-scoped `watch`/`onUnmounted`, so the two mount sites don't share a
 * timer. `connected` only updates when the shared `useDaemon` poller ticks, so
 * the fast-poll actively drives `refresh()` rather than passively reading it.
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

const POLL_MS = 500;
const CEILING_MS = 20_000;

export function useDaemonStart() {
  const { connected, refresh } = useDaemon();

  const starting = ref(false);
  const pendingApproval = ref(false);
  const diagnostics = ref<DaemonDiagnostics | null>(null);

  let pollTimer: ReturnType<typeof setTimeout> | undefined;
  let elapsed = 0;

  function clearPoll(): void {
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = undefined;
    }
  }

  function reset(): void {
    clearPoll();
    starting.value = false;
    pendingApproval.value = false;
    diagnostics.value = null;
  }

  async function diagnose(startError?: string): Promise<void> {
    try {
      const d = await daemonDiagnostics(startError);
      // A registered-but-unapproved daemon isn't a failure: show the approval
      // affordance, not the diagnostics panel.
      if (d.pendingApproval) {
        pendingApproval.value = true;
        diagnostics.value = null;
      } else {
        diagnostics.value = d;
      }
    } catch {
      // Diagnostics gathering itself failed — still surface the start error so
      // the panel isn't empty.
      diagnostics.value = startError ? minimalDiagnostics(startError) : null;
    }
  }

  function beginPolling(startError?: string): void {
    clearPoll();
    elapsed = 0;
    const tick = async (): Promise<void> => {
      if (!starting.value) return;
      await refresh(); // drives the shared poller; updates `connected`
      if (connected.value === true) return; // the watch below clears state
      elapsed += POLL_MS;
      if (elapsed >= CEILING_MS) {
        await diagnose(startError);
        starting.value = false;
        return;
      }
      pollTimer = setTimeout(() => void tick(), POLL_MS);
    };
    pollTimer = setTimeout(() => void tick(), POLL_MS);
  }

  async function start(opts: StartOptions = {}): Promise<void> {
    reset();
    starting.value = true;
    let startError: string | undefined;
    try {
      await startDaemon(opts.nudge ?? false);
    } catch (e) {
      // Launch threw outright (missing sidecar, translocation refusal, register
      // failure) — capture the message and diagnose immediately.
      startError = (e as IpcError).message;
      await diagnose(startError);
      starting.value = false;
      return;
    }
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
      starting.value = false;
      return;
    }
    if (pendingApproval.value) {
      // Can't connect until the user approves; show the affordance, not a panel.
      starting.value = false;
      return;
    }
    beginPolling(startError);
  }

  // Once the daemon is actually reachable, drop any failure/approval UI.
  watch(connected, (c) => {
    if (c === true) {
      starting.value = false;
      pendingApproval.value = false;
      diagnostics.value = null;
      clearPoll();
    }
  });

  onUnmounted(clearPoll);

  return { starting, pendingApproval, diagnostics, start, reset };
}

/** Minimal fallback when `daemon_diagnostics` itself can't run. */
function minimalDiagnostics(startError: string): DaemonDiagnostics {
  return {
    startError,
    hints: [`Start error: ${startError}`],
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
  };
}
