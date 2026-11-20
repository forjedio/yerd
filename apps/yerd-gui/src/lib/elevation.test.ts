import { describe, expect, it } from "vitest";

import type { StatusReport } from "@/ipc/types";
import { needsElevation, portsElevated, privilegedFallback, privilegeItems, privilegeStatus } from "./elevation";

// Only the fields the predicates read matter; the rest of StatusReport is filled
// with inert defaults so the tests stay focused on privilege state. `in` checks
// (not `??`) preserve an explicit `null` tri-state rather than defaulting it.
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

describe("privilegeItems", () => {
  const unbound = { webUnbound: { http: 8080, https: 8443 } };
  const fellBack = { httpReq: 80, httpFell: true };

  function item(r: StatusReport, isMac: boolean, residual: boolean, key: string) {
    const found = privilegeItems(r, isMac, residual).find((i) => i.key === key);
    if (!found) throw new Error(`no item ${key}`);
    return found;
  }

  it("lists trust, resolver and ports in order", () => {
    expect(privilegeItems(mk({}), false, false).map((i) => i.label)).toEqual([
      "Local CA trusted",
      ".test resolver installed",
      "Privileged ports 80/443",
    ]);
  });

  it.each([
    ["web unbound on Linux", mk(unbound), false, { value: false, fixable: true, no: "not serving - elevate to bind 80/443", note: undefined }],
    [
      "web unbound on macOS",
      mk(unbound),
      true,
      {
        value: false,
        fixable: false,
        no: "not serving - set working ports first",
        note: "Yerd couldn't bind its web ports. Set working ports in Settings, then elevate.",
      },
    ],
    ["fell back without redirect", mk(fellBack), false, { value: false, fixable: true, no: "fell back to high ports", note: undefined }],
  ])("ports: %s", (_label, r, isMac, expected) => {
    expect(item(r, isMac, false, "ports")).toMatchObject(expected);
  });

  it.each([
    ["bound directly", mk({}), false, "bound", false],
    ["redirected on macOS", mk({ ...fellBack, portRedirect: true }), true, "redirected", true],
    ["redirect reported on Linux", mk({ ...fellBack, portRedirect: true }), false, "redirected", false],
  ])("ports yes text and revert: %s", (_label, r, isMac, yes, unelevatable) => {
    expect(item(r, isMac, false, "ports")).toMatchObject({ value: true, yes, unelevatable });
  });

  it.each([
    ["trusted, no residual", true, false, true, undefined],
    ["macOS residual system trust", true, true, false, "Trusted system-wide (via Terminal) - run `sudo yerd unelevate trust` to remove it."],
    ["residual flag ignored on Linux", false, true, true, undefined],
  ])("trust revert: %s", (_label, isMac, residual, unelevatable, note) => {
    expect(item(mk({}), isMac, residual, "trust")).toMatchObject({ unelevatable, note });
  });

  it.each([
    ["trust", mk({ trusted: null })],
    ["resolver", mk({ resolver: null })],
  ])("unknown %s state stays null and fixable", (key, r) => {
    expect(item(r, false, false, key)).toMatchObject({ value: null, fixable: true, unelevatable: false });
  });
});

describe("privilegeStatus", () => {
  it.each([
    ["granted", mk({}), "trusted"],
    ["not granted", mk({ trusted: false }), "not trusted"],
    ["unknown", mk({ trusted: null }), "unknown"],
  ])("%s", (_label, r, expected) => {
    const trust = privilegeItems(r, false, false).find((i) => i.key === "trust");
    expect(trust && privilegeStatus(trust)).toBe(expected);
  });
});
