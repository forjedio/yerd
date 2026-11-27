import type { ElevateTarget, StatusReport } from "@/ipc/types";

/**
 * Pure derivations of the daemon's OS-privilege state (CA trust, the .test
 * resolver, privileged ports). `privilegeItems` is the single source of truth:
 * the Welcome screen's EnvironmentCard and the Doctor screen render its rows,
 * and the side nav's attention marker (`needsElevation`) is derived from it, so
 * the surfaces can never disagree about whether elevation is needed.
 */

const PRIVILEGED_PORT_CEILING = 1024;

/** The daemon wanted a privileged web port (< 1024) but fell back to a high one. */
export function privilegedFallback(r: StatusReport): boolean {
  return (
    (r.http.requested < PRIVILEGED_PORT_CEILING && r.http.fell_back) ||
    (r.https.requested < PRIVILEGED_PORT_CEILING && r.https.fell_back)
  );
}

/** Privileged ports are served: either no privileged fallback, or the macOS port forwarder. */
export function portsElevated(r: StatusReport): boolean {
  return !privilegedFallback(r) || r.port_redirect === true;
}

/** One OS privilege as the UI shows it: its tri-state value and which actions apply. */
export interface PrivilegeItem {
  key: string;
  label: string;
  /** `null` when the daemon couldn't determine the state. */
  value: boolean | null;
  fixable: boolean;
  /**
   * Shown only when Yerd established this privilege, so the undo is meaningful.
   * Never derived from `portsElevated` (true even when Yerd installed nothing).
   * Ports is reversible on macOS only (Linux setcap has no clean reverse).
   */
  unelevatable: boolean;
  target: ElevateTarget;
  /** Status text when granted. */
  yes: string;
  /** Status text when not granted. */
  no: string;
  /** Guidance shown in place of an action, e.g. a residual system-wide trust. */
  note?: string;
}

/**
 * The three OS privileges for a status report. `systemTrustRemains` is the
 * macOS flag set when a GUI untrust left a system-wide trust in place: Revert is
 * hidden there (it would do nothing) and a note explains the terminal step.
 *
 * Ports when the daemon bound no web ports (`web_unbound`): on macOS elevation
 * can't help yet (the port forwarder would point at the unbound ports, and the
 * CLI refuses), so the fix is withheld until working ports are set. On Linux the
 * fix is `setcap`, which binds 80/443 directly, so it stays offered.
 */
export function privilegeItems(
  r: StatusReport,
  isMac: boolean,
  systemTrustRemains: boolean,
): PrivilegeItem[] {
  return [
    {
      key: "trust",
      label: "Local CA trusted",
      value: r.ca.trusted_system,
      fixable: r.ca.trusted_system !== true,
      unelevatable: r.ca.trusted_system === true && !(isMac && systemTrustRemains),
      target: "trust",
      yes: "trusted",
      no: "not trusted",
      note:
        isMac && systemTrustRemains
          ? "Trusted system-wide (via Terminal) - run `sudo yerd unelevate trust` to remove it."
          : undefined,
    },
    {
      key: "resolver",
      label: ".test resolver installed",
      value: r.resolver_installed,
      fixable: r.resolver_installed !== true,
      unelevatable: r.resolver_installed === true,
      target: "resolver",
      yes: "installed",
      no: "not installed",
    },
    {
      key: "ports",
      label: "Privileged ports 80/443",
      value: r.web_unbound ? false : portsElevated(r),
      fixable: r.web_unbound ? !isMac : !portsElevated(r),
      unelevatable: r.port_redirect === true && isMac,
      target: "ports",
      yes: r.port_redirect === true && privilegedFallback(r) ? "redirected" : "bound",
      no: r.web_unbound
        ? isMac
          ? "not serving - set working ports first"
          : "not serving - elevate to bind 80/443"
        : "fell back to high ports",
      note:
        r.web_unbound && isMac
          ? "Yerd couldn't bind its web ports. Set working ports in Settings, then elevate."
          : undefined,
    },
  ];
}

/** A privilege's status text: its `yes` or `no` copy, or "unknown" when the state is `null`. */
export function privilegeStatus(item: PrivilegeItem): string {
  if (item.value === true) return item.yes;
  if (item.value === false) return item.no;
  return "unknown";
}

/** True when any OS privilege still offers a fix (any `privilegeItems` row is `fixable`). */
export function needsElevation(r: StatusReport, isMac: boolean): boolean {
  return privilegeItems(r, isMac, false).some((i) => i.fixable);
}
