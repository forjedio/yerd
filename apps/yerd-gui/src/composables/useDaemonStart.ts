import { computed, ref, watch } from "vue";
import { listen } from "@tauri-apps/api/event";

import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { daemonDiagnostics, daemonSelfRepairBusy, IpcError, startDaemon } from "@/ipc/client";
import type { DaemonDiagnostics } from "@/ipc/types";
import { log } from "@/lib/log";

/**
 * Shared "start the daemon, wait for it to actually connect, and diagnose on
 * failure" store - used by onboarding step 1, the DaemonDownHero, and Doctor so
 * all surface the same diagnostics instead of a blind 20 s toast.
 *
 * Module-level **singleton** (like `useDaemon` / `useToast`): the phase, the
 * readiness-poll timer, and the diagnostics live here, not in component setup, so
 * a start kicked off on one screen keeps running and stays visible after the user
 * navigates away (previously the per-instance state was lost on unmount and the
 * Start button reappeared). The active start is mirrored into `useOperations` so
 * the global indicator shows "Starting Yerd…" from anywhere.
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
 * walks those steps; we reflect it in `phase`, which drives a step label on the
 * button ("Installing Daemon" -> "Starting Daemon" -> "Running Daemon"), resetting
 * to idle on completion. The frontend owns the `running` (readiness wait) and
 * `idle` phases; Rust owns the earlier ones.
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

const { connected, refresh } = useDaemon();
const operations = useOperations();

// `phase` is the single source of truth for the click-driven flow; `starting`
// is derived so the existing `:disabled` / spinner bindings keep working
// unchanged. `backgroundBusy` is a second, independent signal for macOS's
// launch-time self-repair thread (`setup_app`), which can re-register/kickstart
// the daemon outside any click - see the `daemon-self-repair` listener below.
// It ORs into `starting` (so the button still shows busy) but deliberately
// stays out of `start()`'s re-entrancy guard, which must keep reading `phase`
// alone: `App.vue`'s auto-start calls `start()` once per mount, and it must
// not be silently swallowed by a same-tick self-repair no-op.
const phase = ref<StartPhase>("idle");
const backgroundBusy = ref(false);
const starting = computed(() => phase.value !== "idle" || backgroundBusy.value);
const activeLabel = computed(() =>
  phase.value === "idle" && backgroundBusy.value ? "Preparing Daemon" : labelFor(phase.value),
);
const pendingApproval = ref(false);
const diagnostics = ref<DaemonDiagnostics | null>(null);

let pollTimer: ReturnType<typeof setTimeout> | undefined;
let elapsed = 0;
// True only from `start()` entry until `startDaemon` resolves - the window in
// which the Rust host owns the phase label.
let acceptRustPhases = false;
let listenerStarted = false;

const OP_ID = "daemon-start";

// Mirror the active start into the global operations registry so the SideNav
// indicator (and anything else) shows it from any screen.
watch(phase, (p) => {
  if (p === "idle") {
    operations.end(OP_ID);
    return;
  }
  const detail = labelFor(p) ?? undefined;
  if (operations.isRunning(OP_ID)) {
    operations.update(OP_ID, { detail });
  } else {
    operations.begin({ id: OP_ID, kind: "daemon-start", label: "Starting Yerd", detail });
  }
});

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

/**
 * Gather and surface daemon-start diagnostics, unless we've since connected (a
 * race where the daemon came up while gathering - don't contradict the connected
 * state). A registered-but-unapproved daemon isn't a failure: show the approval
 * affordance instead of the diagnostics panel. If gathering itself throws, fall
 * back to a minimal but actionable diagnostic rather than a silent dead-end.
 */
async function diagnose(startError?: string): Promise<void> {
  try {
    const d = await daemonDiagnostics(startError);
    if (connected.value === true) return;
    if (d.pendingApproval) {
      pendingApproval.value = true;
      diagnostics.value = null;
    } else {
      diagnostics.value = d;
    }
  } catch {
    if (connected.value === true) return;
    diagnostics.value = minimalDiagnostics(
      startError ?? "The daemon didn't come up, and diagnostics couldn't be gathered.",
    );
  }
}

/**
 * Poll for readiness after a successful launch. The frontend owns this "running"
 * phase, ticking `refresh()` to update `connected`. Each tick re-checks `starting`
 * after the await and bails once connected (the `connected` watch clears the rest
 * of the state) or once the ceiling is hit, where it diagnoses and returns to idle.
 */
function beginPolling(startError?: string): void {
  clearPoll();
  elapsed = 0;
  phase.value = "running";
  const tick = async (): Promise<void> => {
    if (!starting.value) return;
    await refresh();
    if (!starting.value) return;
    if (connected.value === true) return;
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

/**
 * Begin the daemon-start flow (singleton). A second call while one is already in
 * flight is a no-op: the in-flight guard (on `phase`, not the OR'd `starting` -
 * see the field comment above) stops two callers interleaving and clearing each
 * other's shared state, while still letting `App.vue`'s auto-start run through a
 * same-tick `backgroundBusy` window instead of being silently dropped. `phase`
 * goes to "starting" synchronously so the spinner shows on click (the macOS plan
 * can spend seconds probing launchctl before the first phase event). Once
 * `startDaemon` returns, `acceptRustPhases` is dropped so a straggler phase event
 * can't flip the label after the frontend owns running/idle; a launch throw
 * (missing sidecar, translocation refusal, register failure) diagnoses
 * immediately. `nudge` defaults to true (open Login Items on a pending
 * approval); only onboarding passes false. A throwing `beforeProbe` is treated
 * as not-pending-approval.
 */
async function start(opts: StartOptions = {}): Promise<void> {
  if (phase.value !== "idle") return;
  reset();
  phase.value = "starting";
  acceptRustPhases = true;
  log.info("daemon start requested");
  let startError: string | undefined;
  try {
    await startDaemon(opts.nudge ?? true);
  } catch (e) {
    acceptRustPhases = false;
    startError = (e as IpcError).message;
    log.error(`daemon start failed: ${startError}`);
    await diagnose(startError);
    phase.value = "idle";
    return;
  }
  acceptRustPhases = false;
  if (opts.beforeProbe) {
    try {
      pendingApproval.value = await opts.beforeProbe();
    } catch {
      pendingApproval.value = false;
    }
  }
  await refresh();
  if (connected.value === true) {
    phase.value = "idle";
    return;
  }
  if (pendingApproval.value) {
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

/**
 * Register the always-on Rust phase listener, lazily on first use (not at import,
 * so a non-Tauri context that imports this module doesn't call `listen`). Gated by
 * `acceptRustPhases` so only an in-flight `start()` reflects the event. The
 * singleton lives for the app's lifetime, so the unlisten handle is intentionally
 * dropped; if events are unavailable (non-Tauri/test context) the listener flag is
 * reset so phases simply won't update.
 *
 * Also registers the `daemon-self-repair` listener (macOS launch-time self-repair
 * thread) and seeds `backgroundBusy` from the current flag right after, in case
 * the thread was already mid-flight before this listener attached. The seed is
 * guarded by `backgroundBusySeen`: once any event has arrived, it wins outright -
 * without that, the seed's IPC round-trip could sample the flag while still
 * `true`, race with the thread's own terminal `false` event arriving first, and
 * then overwrite `backgroundBusy` back to `true` with no further event ever
 * coming to correct it.
 */
async function ensureListener(): Promise<void> {
  if (listenerStarted) return;
  listenerStarted = true;
  let backgroundBusySeen = false;
  try {
    await listen<string>("daemon-start-phase", (e) => {
      if (!acceptRustPhases) return;
      const p = e.payload;
      if (p === "installing" || p === "upgrading" || p === "starting") {
        phase.value = p;
        log.debug(`daemon start phase: ${p}`);
      }
    });
    await listen<boolean>("daemon-self-repair", (e) => {
      backgroundBusySeen = true;
      backgroundBusy.value = e.payload;
      log.debug(`daemon self-repair: ${e.payload ? "busy" : "idle"}`);
    });
  } catch {
    listenerStarted = false;
    return;
  }
  try {
    const busy = await daemonSelfRepairBusy();
    if (!backgroundBusySeen) backgroundBusy.value = busy;
  } catch {
    /* non-fatal: non-macOS or IPC unavailable - backgroundBusy stays false */
  }
}

export function useDaemonStart() {
  void ensureListener();
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
