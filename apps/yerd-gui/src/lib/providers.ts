/**
 * Pure, display-only helpers for the Tunnel providers sheet. No I/O; inputs
 * are the daemon's `list_tunnel_providers` reply.
 */
import type { ProviderReadiness, TunnelProvider, TunnelProviderStatus } from "@/ipc/types";

/** Providers in display order. */
export const PROVIDER_ORDER: readonly TunnelProvider[] = ["cloudflare", "ngrok", "zrok"];

const NAMES: Record<TunnelProvider, string> = {
  cloudflare: "Cloudflare",
  ngrok: "ngrok",
  zrok: "zrok",
};

/** A provider's display name. */
export function providerName(provider: TunnelProvider): string {
  return NAMES[provider];
}

/** The status line under a provider. */
export function readinessLabel(readiness: ProviderReadiness): string {
  if (readiness === "ready") return "Ready";
  if (readiness === "needs_setup") return "Needs setup";
  return "Not installed";
}

/** The status dot's colour class: green ready, amber otherwise. */
export function readinessDot(readiness: ProviderReadiness): string {
  return readiness === "ready" ? "bg-success" : "bg-warning";
}

/** The binary headline: `ngrok 3.18.4`, or just the name when the version is unknown. */
export function binaryLine(status: TunnelProviderStatus, name: string): string {
  return status.version ? `${name} ${status.version}` : name;
}

/** Where the binary came from. */
export function sourceLabel(status: TunnelProviderStatus): string {
  if (status.source === "managed") return "Yerd's copy";
  if (status.source === "system") return "Found on PATH";
  return "Not installed";
}

/** Whether Tooling has something to do: install the binary or update Yerd's copy. */
export function needsTooling(status: TunnelProviderStatus): boolean {
  if (status.readiness === "not_installed") return true;
  return status.source === "managed" && Boolean(status.update_available);
}

/** A Share page provider card's status: Cloudflare `Connected` once logged in,
 *  ngrok `Token saved`, zrok `Enabled`, else `Not installed` / `Not set up`. */
export function providerCardStatus(status: TunnelProviderStatus): string {
  if (status.readiness === "not_installed") return "Not installed";
  if (status.provider === "cloudflare") return status.configured ? "Connected" : readinessLabel(status.readiness);
  if (!status.configured) return "Not set up";
  return status.provider === "ngrok" ? "Token saved" : "Enabled";
}

/** A provider card's status dot class. */
export function providerCardDot(status: TunnelProviderStatus): string {
  return readinessDot(status.readiness);
}

/** Options for the "Default for new shares" select. */
export const DEFAULT_OPTIONS: readonly { value: TunnelProvider; label: string }[] = PROVIDER_ORDER.map(
  (value) => ({ value, label: providerName(value) }),
);
