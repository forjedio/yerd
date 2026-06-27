import { useDaemon } from "@/composables/useDaemon";
import {
  IpcError,
  restartDaemon,
  setDnsPort,
  setDumpsPort,
  setFallbackPorts,
  setMailPort,
} from "@/ipc/client";

/**
 * Shared logic for editing the daemon's ports (rootless HTTP/HTTPS fallback,
 * DNS, mail, dumps), used by BOTH the Settings "Application Ports" editor and
 * the onboarding degraded-port panel so the validate → save → restart → re-check
 * flow can't diverge between them.
 *
 * The restart detection is deliberately careful (see `applyAndRestart`): the
 * shared `useDaemon` poller runs every 4 s, so a fast re-exec can complete
 * inside one gap - meaning a passive watch on `connected` could never observe
 * the drop, and `connected === true` holds at *both* ends of a restart. We
 * therefore actively drive `refresh()` and key completion on a change in the
 * daemon's per-process `boot_id` (the re-exec preserves the pid and
 * `uptime_secs` has only one-second granularity, so neither is reliable).
 */

/** Minimum for the rootless HTTP/HTTPS fallback pair - a privileged port like
 *  80/443 would itself need elevation, which the fallback exists to avoid. */
export const MIN_PORT = 1024;
export const MAX_PORT = 65535;
/** Minimum for the DNS/mail/dumps loopback ports, which may legitimately be
 *  below 1024 (the daemon binds them directly, no elevation involved). */
export const MIN_LOOPBACK_PORT = 1;

const POLL_MS = 500;
const CEILING_MS = 20_000;

export interface SaveResult {
  /** True once the daemon came back up without the changed port still degraded. */
  ok: boolean;
  /** A user-facing reason when `ok` is false. */
  message?: string;
}

/** A set of port changes to apply. Omit a field to leave that port untouched. */
export interface PortChanges {
  /** Rootless HTTP/HTTPS fallback pair. */
  web?: { http: number; https: number };
  /** DNS responder port. */
  dns?: number;
  /** Mail-capture SMTP port. */
  mail?: number;
  /** Dump-server port. */
  dumps?: number;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function useFallbackPorts() {
  const { report, connected, refresh } = useDaemon();

  /**
   * Validate a candidate fallback pair. Returns an error string to show the
   * user, or `null` when the pair is acceptable. Mirrors the daemon's own
   * `validate()`.
   */
  function validate(http: number, https: number): string | null {
    for (const p of [http, https]) {
      if (!Number.isInteger(p) || p < MIN_PORT || p > MAX_PORT) {
        return `The HTTP and HTTPS ports must be whole numbers between ${MIN_PORT} and ${MAX_PORT} - a privileged port like 80/443 would need elevation, which the fallback exists to avoid.`;
      }
    }
    if (http === https) {
      return "The HTTP and HTTPS ports must be different.";
    }
    return null;
  }

  /** Validate a single loopback port (DNS/mail/dumps): a whole number 1-65535. */
  function validateLoopback(label: string, port: number): string | null {
    if (!Number.isInteger(port) || port < MIN_LOOPBACK_PORT || port > MAX_PORT) {
      return `The ${label} port must be a whole number between ${MIN_LOOPBACK_PORT} and ${MAX_PORT}.`;
    }
    return null;
  }

  /**
   * Persist a set of port changes and restart the daemon, then wait for it to
   * come back and report whether the changed ports are now bound. The daemon
   * rejects invalid changes (thrown `IpcError`) - surfaced as `{ ok: false }`
   * with its message; the caller never has to try/catch.
   */
  async function applyAndRestart(changes: PortChanges): Promise<SaveResult> {
    // 1. Pre-validate everything BEFORE any IPC call, so a bad value never
    //    persists a partial change.
    if (changes.web) {
      const err = validate(changes.web.http, changes.web.https);
      if (err) return { ok: false, message: err };
    }
    if (changes.dns != null) {
      const err = validateLoopback("DNS", changes.dns);
      if (err) return { ok: false, message: err };
    }
    if (changes.mail != null) {
      const err = validateLoopback("mail", changes.mail);
      if (err) return { ok: false, message: err };
    }
    if (changes.dumps != null) {
      const err = validateLoopback("dumps", changes.dumps);
      if (err) return { ok: false, message: err };
    }

    // Snapshot the current process so we can tell the restarted daemon apart
    // from the still-running one. `boot_id` is the reliable key; fall back to
    // `uptime_secs` only if an older daemon omits it.
    const baselineBootId = report.value?.boot_id ?? null;
    const baselineUptime = report.value?.uptime_secs ?? Number.MAX_SAFE_INTEGER;

    // 2. Apply the setters. Order matters: `setDumpsPort` is the only setter
    //    that test-binds at call time (can throw PortInUse), so do it FIRST -
    //    a busy dumps port then aborts before any other config is written to
    //    disk. The rest only clone→validate→save (fail only on a disk error).
    try {
      if (changes.dumps != null) await setDumpsPort(changes.dumps);
      if (changes.web) await setFallbackPorts(changes.web.http, changes.web.https);
      if (changes.dns != null) await setDnsPort(changes.dns);
      if (changes.mail != null) await setMailPort(changes.mail);
    } catch (e) {
      return { ok: false, message: (e as IpcError).message };
    }

    // 3. Restart and wait for the new process.
    try {
      await restartDaemon();
    } catch {
      // The restart tears down the socket, so `restartDaemon()` can reject with
      // a dropped-connection error *even though the restart is underway*. The
      // boot_id poll below is the authoritative completion check, so don't bail
      // here - a daemon that genuinely didn't restart simply makes it time out.
    }

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

      // 4. Scope the degraded check to what actually changed: a mail/dumps-only
      //    save must not fail just because an *unrelated, pre-existing*
      //    web/DNS conflict is still present.
      if (changes.web && report.value.web_unbound != null) {
        const u = report.value.web_unbound;
        return {
          ok: false,
          message: `Yerd still couldn't bind ports ${u.http}/${u.https} - they may be in use too. Try different ports.`,
        };
      }
      if (changes.dns != null && report.value.dns_unbound != null) {
        return {
          ok: false,
          message: `Yerd still couldn't bind the DNS port ${report.value.dns_unbound} - it may be in use too. Try a different port.`,
        };
      }
      // Mail has no test-bind and no dedicated `*_unbound` field; detect a busy
      // port via the SMTP listener state (`enabled && !listening` = port busy).
      if (
        changes.mail != null &&
        report.value.mail?.enabled === true &&
        report.value.mail.listening === false
      ) {
        return {
          ok: false,
          message: `Yerd couldn't bind the mail port ${changes.mail} - it may be in use. Try a different port.`,
        };
      }
      return { ok: true };
    }
    return {
      ok: false,
      message: "Timed out waiting for the daemon to restart. Check Settings, then try again.",
    };
  }

  return {
    validate,
    validateLoopback,
    applyAndRestart,
    MIN_PORT,
    MAX_PORT,
  };
}
