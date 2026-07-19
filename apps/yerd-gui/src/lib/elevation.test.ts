import { describe, expect, it } from "vitest";

import type { StatusReport } from "@/ipc/types";
import { needsElevation, portsElevated, privilegedFallback } from "./elevation";

// Only the fields the predicates read matter; the rest of StatusReport is filled
// with inert defaults so the tests stay focused on privilege state.
function mk(over: {
  httpReq?: number;
  httpFell?: boolean;
  httpsReq?: number;
  httpsFell?: boolean;
  portRedirect?: boolean | null;
  trusted?: boolean | null;
  resolver?: boolean | null;
  webUnbound?: { http: number; https: number } | null;
}): StatusReport {
  // `in` checks, not `??`: an explicit `null` (tri-state "undeterminable") must
  // survive rather than fall through to the `true` default.
  return {
    http: { requested: over.httpReq ?? 80, bound: 80, fell_back: over.httpFell ?? false },
    https: { requested: over.httpsReq ?? 443, bound: 443, fell_back: over.httpsFell ?? false },
    port_redirect: over.portRedirect,
    ca: { path: "", fingerprint: "", trusted_system: "trusted" in over ? over.trusted : true },
    resolver_installed: "resolver" in over ? over.resolver : true,
    web_unbound: over.webUnbound,
  } as StatusReport;
}

describe("privilegedFallback", () => {
  it.each([
    ["bound its requested privileged ports", mk({}), false],
    ["fell back from privileged http", mk({ httpReq: 80, httpFell: true }), true],
    ["fell back from privileged https", mk({ httpsReq: 443, httpsFell: true }), true],
    ["fell back but from a high port", mk({ httpReq: 8080, httpFell: true }), false],
  ])("%s → %s", (_label, r, expected) => {
    expect(privilegedFallback(r)).toBe(expected);
  });
});

describe("portsElevated", () => {
  it.each([
    ["no fallback", mk({}), true],
    ["privileged fallback, no redirect", mk({ httpReq: 80, httpFell: true }), false],
    ["privileged fallback but macOS redirect carries them", mk({ httpReq: 80, httpFell: true, portRedirect: true }), true],
  ])("%s → %s", (_label, r, expected) => {
    expect(portsElevated(r)).toBe(expected);
  });
});

describe("needsElevation", () => {
  it.each([
    ["all privileges satisfied", mk({}), false, false],
    ["all satisfied (macOS)", mk({}), true, false],
    ["CA not trusted", mk({ trusted: false }), false, true],
    ["CA trust undeterminable (null, not true)", mk({ trusted: null }), false, true],
    ["resolver missing", mk({ resolver: false }), false, true],
    ["ports fell back to high ports", mk({ httpReq: 80, httpFell: true }), false, true],
    ["ports fell back but redirected on macOS", mk({ httpReq: 80, httpFell: true, portRedirect: true }), true, false],
    ["web unbound on Linux is fixable via setcap", mk({ webUnbound: { http: 8080, https: 8443 } }), false, true],
    ["web unbound on macOS is not fixable yet", mk({ webUnbound: { http: 8080, https: 8443 } }), true, false],
  ])("%s → %s", (_label, r, isMac, expected) => {
    expect(needsElevation(r, isMac)).toBe(expected);
  });
});
