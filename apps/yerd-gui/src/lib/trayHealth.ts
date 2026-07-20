/**
 * Overall tray health (green / amber / red) derived from a StatusReport.
 *
 * `deriveTrayHealth` mirrors `apps/yerd-gui/src-tauri/src/tray_health.rs`.
 *
 * Service rows are **not** shared with the native menu: `trayServiceRows` here
 * lists only **PHP pools** that are running or failed (Herd-style tray panel).
 * `tray_health::service_rows` in Rust lists Proxy + every pool + every managed
 * instance for the read-only native menu.
 */
import type { PhpVersion, StatusReport, PhpPoolStatus } from "@/ipc/types";

export type TrayHealth = "ok" | "warn" | "bad";

const PRIVILEGED_PORT_CEILING = 1024;

export function deriveTrayHealth(report: StatusReport | null | undefined): TrayHealth {
  if (!report) return "bad";

  if (
    report.web_unbound ||
    report.dns_unbound != null ||
    report.foreign_web_listener === true ||
    (report.php ?? []).some((p) => p.state === "failed") ||
    (report.services ?? []).some((s) => s.state === "failed")
  ) {
    return "bad";
  }

  const portsDegraded = portsFellPrivileged(report) && report.port_redirect !== true;
  const caBad = report.ca?.trusted_system === false;
  const resolverBad = report.resolver_installed === false;
  const enabledStopped = (report.services ?? []).some(
    (s) => s.enabled && s.state === "stopped",
  );

  if (portsDegraded || caBad || resolverBad || enabledStopped) {
    return "warn";
  }

  return "ok";
}

function portsFellPrivileged(report: StatusReport): boolean {
  return (
    (report.http.requested < PRIVILEGED_PORT_CEILING && report.http.fell_back) ||
    (report.https.requested < PRIVILEGED_PORT_CEILING && report.https.fell_back)
  );
}

export type TrayServiceKind = "proxy" | "php_pool" | "managed";

export type TrayServiceRunState = "running" | "stopped" | "failed";

export interface TrayServiceRow {
  id: string;
  label: string;
  health: TrayHealth;
  kind: TrayServiceKind;
  state: TrayServiceRunState;
  canStart: boolean;
  canStop: boolean;
  canRestart: boolean;
  /** Set for php_pool rows so restart can target the right FPM pool. */
  phpVersion?: PhpVersion;
}

export function trayServiceRows(report: StatusReport): TrayServiceRow[] {
  const rows: TrayServiceRow[] = [];

  const php: PhpPoolStatus[] = report.php ?? [];
  for (const pool of php) {
    if (!isActiveRunState(pool.state)) continue;
    const health: TrayHealth = pool.state === "failed" ? "bad" : "ok";
    rows.push({
      id: `php:${pool.version}`,
      label: `PHP ${pool.version}`,
      health,
      kind: "php_pool",
      state: pool.state,
      canStart: false,
      canStop: false,
      canRestart: true,
      phpVersion: pool.version,
    });
  }

  return rows;
}

function isActiveRunState(state: TrayServiceRunState): boolean {
  return state === "running" || state === "failed";
}

/** Tailwind / StatusPill tone for a health value. */
export function trayHealthTone(h: TrayHealth): "ok" | "warn" | "bad" {
  return h;
}

/** Whether cert/CA trust looks healthy (v2 cert indicator). */
export function certHealth(report: StatusReport | null | undefined): TrayHealth {
  if (!report) return "bad";
  if (report.ca?.trusted_system === false) return "bad";
  if (report.ca?.trusted_system == null) return "warn";
  if (report.ca?.php_trusts_ca === false) return "warn";
  return "ok";
}

/** Structured alert items for the tray Activity feed. */
export interface TrayAlert {
  id: string;
  title: string;
  detail?: string;
  /** Navigate target inside the main window, or a special action key. */
  action: "doctor" | "services" | "settings" | "mail";
  actionLabel: string;
  tone: TrayHealth;
}

export function trayAlerts(report: StatusReport | null | undefined): TrayAlert[] {
  if (!report) {
    return [
      {
        id: "daemon-down",
        title: "Daemon unreachable",
        detail: "Yerd is not responding",
        action: "doctor",
        actionLabel: "Doctor",
        tone: "bad",
      },
    ];
  }

  const alerts: TrayAlert[] = [];

  if (report.web_unbound || report.dns_unbound != null || report.foreign_web_listener === true) {
    alerts.push({
      id: "web-unbound",
      title: "Proxy issues",
      detail: "Web or DNS is unbound, or a foreign listener is on the ports",
      action: "doctor",
      actionLabel: "Doctor",
      tone: "bad",
    });
  }

  if ((report.php ?? []).some((p) => p.state === "failed")) {
    alerts.push({
      id: "php-failed",
      title: "PHP pool failed",
      detail: "One or more FPM pools crashed",
      action: "services",
      actionLabel: "Services",
      tone: "bad",
    });
  }

  if ((report.services ?? []).some((s) => s.state === "failed")) {
    alerts.push({
      id: "svc-failed",
      title: "Service failed",
      detail: "A managed service is in a failed state",
      action: "services",
      actionLabel: "Services",
      tone: "bad",
    });
  }

  if (portsFellPrivileged(report) && report.port_redirect !== true) {
    alerts.push({
      id: "ports-degraded",
      title: "Privileged ports unavailable",
      detail: "Using fallback ports without a redirect",
      action: "doctor",
      actionLabel: "Doctor",
      tone: "warn",
    });
  }

  if (report.ca?.trusted_system === false) {
    alerts.push({
      id: "ca-untrusted",
      title: "CA not trusted",
      detail: "HTTPS certificates may show warnings",
      action: "settings",
      actionLabel: "Settings",
      tone: "bad",
    });
  } else if (report.ca?.trusted_system == null) {
    alerts.push({
      id: "ca-unknown",
      title: "CA trust unknown",
      action: "settings",
      actionLabel: "Settings",
      tone: "warn",
    });
  }

  if (report.resolver_installed === false) {
    alerts.push({
      id: "resolver-missing",
      title: "DNS resolver not installed",
      action: "doctor",
      actionLabel: "Doctor",
      tone: "warn",
    });
  }

  const enabledStopped = (report.services ?? []).filter(
    (s) => s.enabled && s.state === "stopped",
  );
  if (enabledStopped.length > 0) {
    alerts.push({
      id: "enabled-stopped",
      title:
        enabledStopped.length === 1
          ? `${enabledStopped[0]!.display_name} is stopped`
          : `${enabledStopped.length} services stopped`,
      detail: "Enabled services that are not running",
      action: "services",
      actionLabel: "Services",
      tone: "warn",
    });
  }

  return alerts.slice(0, 4);
}
