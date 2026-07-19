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
 * True when any OS privilege still needs a fix: CA trust, the .test resolver, or
 * privileged ports. Mirrors EnvironmentCard's per-row `fixable` (its `anyFixable`
 * aggregate). The ports branch depends on the host: when the daemon bound no web
 * ports at all (`web_unbound`), elevation can only help on Linux (setcap binds
 * 80/443 directly); macOS needs working ports set first, so it isn't fixable yet.
 */
export function needsElevation(r: StatusReport, isMac: boolean): boolean {
  return (
    r.ca.trusted_system !== true ||
    r.resolver_installed !== true ||
    (r.web_unbound ? !isMac : !portsElevated(r))
  );
}
