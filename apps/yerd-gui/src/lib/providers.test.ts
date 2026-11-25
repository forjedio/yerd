import { describe, expect, it } from "vitest";

import type { TunnelProviderStatus } from "@/ipc/types";
import {
  binaryLine,
  DEFAULT_OPTIONS,
  needsTooling,
  PROVIDER_ORDER,
  providerCardDot,
  providerCardStatus,
  providerName,
  readinessDot,
  readinessLabel,
  sourceLabel,
} from "./providers";

function status(extra: Partial<TunnelProviderStatus> = {}): TunnelProviderStatus {
  return { provider: "ngrok", readiness: "ready", configured: true, ...extra };
}

describe("providers", () => {
  it.each([
    ["cloudflare", "Cloudflare"],
    ["ngrok", "ngrok"],
    ["zrok", "zrok"],
  ] as const)("names %s", (provider, name) => {
    expect(providerName(provider)).toBe(name);
  });

  it.each([
    ["ready", "Ready", "bg-success"],
    ["needs_setup", "Needs setup", "bg-warning"],
    ["not_installed", "Not installed", "bg-warning"],
  ] as const)("labels %s", (readiness, label, dot) => {
    expect(readinessLabel(readiness)).toBe(label);
    expect(readinessDot(readiness)).toBe(dot);
  });

  it.each([
    [status({ version: "3.18.4" }), "ngrok 3.18.4"],
    [status(), "ngrok"],
  ])("binary line %#", (s, expected) => {
    expect(binaryLine(s, "ngrok")).toBe(expected);
  });

  it.each([
    [status({ source: "managed" }), "Yerd's copy"],
    [status({ source: "system" }), "Found on PATH"],
    [status({ readiness: "not_installed" }), "Not installed"],
  ])("source label %#", (s, expected) => {
    expect(sourceLabel(s)).toBe(expected);
  });

  it.each([
    [status({ readiness: "not_installed" }), true],
    [status({ source: "managed", update_available: "3.19.0" }), true],
    [status({ source: "system", update_available: "3.19.0" }), false],
    [status({ source: "managed" }), false],
  ])("needs Tooling %#", (s, expected) => {
    expect(needsTooling(s)).toBe(expected);
  });

  it.each([
    [status({ provider: "cloudflare", configured: true }), "Connected", "bg-success"],
    [status({ provider: "cloudflare", configured: false }), "Ready", "bg-success"],
    [status({ provider: "ngrok" }), "Token saved", "bg-success"],
    [status({ provider: "zrok" }), "Enabled", "bg-success"],
    [status({ provider: "zrok", readiness: "needs_setup", configured: false }), "Not set up", "bg-warning"],
    [status({ provider: "ngrok", readiness: "not_installed", configured: true }), "Not installed", "bg-warning"],
  ])("provider card status %#", (s, label, dot) => {
    expect(providerCardStatus(s)).toBe(label);
    expect(providerCardDot(s)).toBe(dot);
  });

  it("offers every provider as a default", () => {
    expect(PROVIDER_ORDER).toEqual(["cloudflare", "ngrok", "zrok"]);
    expect(DEFAULT_OPTIONS.map((o) => o.label)).toEqual(["Cloudflare", "ngrok", "zrok"]);
  });
});
