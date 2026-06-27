import { ref } from "vue";

import { IpcError, markOnboarded, setupState, status } from "@/ipc/client";

// First-run welcome-journey state, shared as a module-level singleton so the
// root (App.vue, which runs the probe + the splash gate) and the journey view
// (WelcomeView, which finishes it) agree without prop-drilling.
//
// The journey is shown only on a never-set-up machine: not previously onboarded,
// no existing config/PHP/service, and the daemon currently unreachable. A real
// `yerd uninstall` wipes the config dir (which holds the onboarding flag), so the
// journey reappears afterwards - a plain Trash-drag of the app leaves the setup
// intact and correctly does not.

/** True until the one-time probe resolves; gates the first paint. */
const probing = ref(true);
/** True when the first-run journey should replace the normal app shell. */
const needsOnboarding = ref(false);
let probed = false;
// The actual reachability from the one probe, so cached calls report the truth
// (not `!needsOnboarding`, which is false even when the daemon is down but the
// machine is already set up).
let lastReachable = false;

/** Is the daemon reachable now? Mirrors useDaemon: only an *unreachable* socket
 *  counts as down; a typed daemon error still means it's up. */
async function daemonReachable(): Promise<boolean> {
  try {
    await status();
    return true;
  } catch (e) {
    return !(e instanceof IpcError && e.unreachable);
  }
}

export function useOnboarding() {
  /**
   * Run the one-time first-run probe. Sets `needsOnboarding`, clears `probing`,
   * and returns whether the daemon was reachable so the caller can decide
   * whether to auto-start it (the journey owns starting it otherwise).
   */
  async function probe(): Promise<{ reachable: boolean }> {
    if (probed) return { reachable: lastReachable };
    probed = true;
    const reachable = await daemonReachable();
    lastReachable = reachable;
    if (reachable) {
      needsOnboarding.value = false;
    } else {
      try {
        const s = await setupState();
        needsOnboarding.value = !s.onboarded && !s.isSetUp;
      } catch {
        // Can't determine setup state → don't trap the user in the journey.
        needsOnboarding.value = false;
      }
    }
    probing.value = false;
    return { reachable };
  }

  /**
   * Re-enter the welcome journey on demand (e.g. from the Overview banner shown
   * when the environment looks empty). App.vue renders `WelcomeView` reactively
   * off `needsOnboarding`, so flipping it here swaps the wizard in immediately.
   * The journey handles an already-running daemon / already-onboarded machine
   * fine: step 1 reads "Running" and `finish()` → `markOnboarded()` is idempotent.
   */
  function relaunch(): void {
    needsOnboarding.value = true;
  }

  /** Persist completion and leave the journey. Best-effort on the persist. */
  async function finish(): Promise<void> {
    try {
      await markOnboarded();
    } catch {
      // Non-fatal: the daemon being up after step 1 already makes the journey
      // not reappear next launch even if the flag write failed.
    }
    needsOnboarding.value = false;
  }

  return { probing, needsOnboarding, probe, relaunch, finish };
}
