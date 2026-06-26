import { useDaemon } from "@/composables/useDaemon";
import { IpcError, restartDaemon, setFallbackPorts } from "@/ipc/client";

/**
 * Shared logic for editing the daemon's rootless HTTP/HTTPS fallback ports,
 * used by BOTH the Settings editor and the onboarding degraded-port panel so
 * the validate → save → restart → re-check flow can't diverge between them.
 *
 * The restart detection is deliberately careful (see `saveAndRestart`): the
 * shared `useDaemon` poller runs every 4 s, so a fast re-exec can complete
 * inside one gap - meaning a passive watch on `connected` could never observe
 * the drop, and `connected === true` holds at *both* ends of a restart. We
 * therefore actively drive `refresh()` and key completion on a change in the
 * daemon's per-process `boot_id` (the re-exec preserves the pid and
 * `uptime_secs` has only one-second granularity, so neither is reliable).
 */

export const MIN_PORT = 1024;
export const MAX_PORT = 65535;

const POLL_MS = 500;
const CEILING_MS = 20_000;

export interface SaveResult {
  /** True once the daemon came back up serving (no longer degraded). */
  ok: boolean;
  /** A user-facing reason when `ok` is false. */
  message?: string;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function useFallbackPorts() {
  const { report, connected, refresh } = useDaemon();

  /**
   * Validate a candidate pair. Returns an error string to show the user, or
   * `null` when the pair is acceptable. Mirrors the daemon's own `validate()`.
   */
  function validate(http: number, https: number): string | null {
    for (const p of [http, https]) {
      if (!Number.isInteger(p) || p < MIN_PORT || p > MAX_PORT) {
        return `Ports must be whole numbers between ${MIN_PORT} and ${MAX_PORT} - a privileged port like 80/443 would need elevation, which the fallback exists to avoid.`;
      }
    }
    if (http === https) {
      return "The HTTP and HTTPS ports must be different.";
    }
    return null;
  }

  /**
   * Persist the new fallback ports and restart the daemon, then wait for it to
   * come back and report whether it is now serving. The daemon rejects invalid
   * or elevated changes (thrown `IpcError`) - surfaced as `{ ok: false }` with
   * its message; the caller never has to try/catch.
   */
  async function saveAndRestart(http: number, https: number): Promise<SaveResult> {
    const local = validate(http, https);
    if (local) return { ok: false, message: local };

    // Snapshot the current process id so we can tell the restarted daemon apart
    // from the still-running one. `boot_id` is the reliable key; fall back to
    // `uptime_secs` only if an older daemon omits it.
    const baselineBootId = report.value?.boot_id ?? null;
    const baselineUptime = report.value?.uptime_secs ?? Number.MAX_SAFE_INTEGER;

    try {
      await setFallbackPorts(http, https);
    } catch (e) {
      return { ok: false, message: (e as IpcError).message };
    }
    try {
      await restartDaemon();
    } catch {
      // The restart tears down the socket, so `restartDaemon()` can reject with
      // a dropped-connection error *even though the restart is underway* (the
      // daemon may close the connection around its re-exec). The boot_id poll
      // below is the authoritative completion check, so don't bail here - mirror
      // GeneralView's restart flow, which also tolerates the throw. A daemon that
      // genuinely didn't restart simply makes the poll time out.
    }

    // Actively poll until the *restarted* daemon answers (fresh boot_id), then
    // read its degraded state. Bounded so a daemon that never returns can't hang.
    let elapsed = 0;
    while (elapsed < CEILING_MS) {
      await sleep(POLL_MS);
      elapsed += POLL_MS;
      await refresh();
      if (connected.value !== true || !report.value) continue;
      const restarted =
        baselineBootId !== null
          ? report.value.boot_id != null && report.value.boot_id !== baselineBootId
          : report.value.uptime_secs < baselineUptime;
      if (!restarted) continue;
      if (report.value.web_unbound == null) {
        return { ok: true };
      }
      const u = report.value.web_unbound;
      return {
        ok: false,
        message: `Yerd still couldn't bind ports ${u.http}/${u.https} - they may be in use too. Try different ports.`,
      };
    }
    return {
      ok: false,
      message: "Timed out waiting for the daemon to restart. Check Settings, then try again.",
    };
  }

  return { validate, saveAndRestart, MIN_PORT, MAX_PORT };
}
