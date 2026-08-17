import type { StatusReport } from "@/ipc/types";

/**
 * Pure predicates for the daemon's OS-privilege state, shared between the Doctor
 * page's EnvironmentCard (which renders the full fix/revert table) and the side
 * nav (which shows an amber attention marker when anything still needs a
 * privileged fix). Keeping them here is the single source of truth so the two
 * surfaces can never disagree about whether elevation is needed.
 */

const PRIVILEGED_PORT_CEILING = 1024;

/** The daemon wanted a privileged web port (< 1024) but fell back to a high one. */
export function privilegedFallback(r: StatusReport): boolean {
  return (
    (r.http.requested < PRIVILEGED_PORT_CEILING && r.http.fell_back) ||
    (r.https.requested < PRIVILEGED_PORT_CEILING && r.https.fell_back)
  );
}

/** Privileged ports are served: either no privileged fallback, or macOS pf redirect. */
export function portsElevated(r: StatusReport): boolean {
  return !privilegedFallback(r) || r.port_redirect === true;
}

/**
 * Whether the privileged-ports row still needs a fix that elevation can actually
 * deliver. The answer depends on the host: when the daemon bound no web ports at
 * all (`web_unbound`), elevation only helps on Linux (setcap binds 80/443
 * directly); macOS needs working ports set first, so it isn't fixable yet.
 *
 * Windows is never fixable: sub-1024 binds are unprivileged there, so
 * `yerd elevate ports` deliberately prints a skip note and exits 0. Offering the
 * fix would report success, change nothing, and leave the attention marker lit.
 * A Windows fallback means another process holds the port, which no privilege
 * can resolve.
 */
export function portsNeedElevation(r: StatusReport, isMac: boolean, isWindows: boolean): boolean {
  if (isWindows) return false;
  return r.web_unbound ? !isMac : !portsElevated(r);
}

/**
 * True when any OS privilege still needs a fix: CA trust, the .test resolver, or
 * privileged ports. Mirrors EnvironmentCard's per-row `fixable` (its `anyFixable`
 * aggregate).
 */
export function needsElevation(r: StatusReport, isMac: boolean, isWindows = false): boolean {
  return (
    r.ca.trusted_system !== true ||
    r.resolver_installed !== true ||
    portsNeedElevation(r, isMac, isWindows)
  );
}
