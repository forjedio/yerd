/**
 * Shared start/stop predicates for managed service instances.
 * Used by ServicesView and the tray panel so the rules stay in sync.
 */
import type { ServiceStatus } from "@/ipc/types";

/** A per-site instance (e.g. Reverb) has no installed version but is still startable. */
export function isPerSiteService(s: ServiceStatus): boolean {
  return !!s.site;
}

/** Installed engine on disk, or a configured per-site instance (matches ServicesView). */
export function isInstalledService(s: ServiceStatus): boolean {
  return s.installed_versions.length > 0 || isPerSiteService(s);
}

export function canStartService(s: ServiceStatus): boolean {
  return (s.installed_versions.length > 0 || isPerSiteService(s)) && s.state !== "running";
}

export function canStopService(s: ServiceStatus): boolean {
  return s.state === "running" || s.state === "failed";
}
