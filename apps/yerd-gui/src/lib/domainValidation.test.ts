import { describe, expect, it } from "vitest";

import { isUnderTld, validateDomainShape } from "./domainValidation";

describe("validateDomainShape", () => {
  it("accepts exact, subdomain, and single-label wildcard FQDNs", () => {
    expect(validateDomainShape("foo.test")).toBeNull();
    expect(validateDomainShape("api.foo.test")).toBeNull();
    expect(validateDomainShape("*.foo.test")).toBeNull();
    expect(validateDomainShape("*.api.foo.test")).toBeNull();
  });

  it("accepts a trailing dot (mirrors the CLI's strip_suffix)", () => {
    expect(validateDomainShape("foo.test.")).toBeNull();
  });

  it("lowercases before checking", () => {
    expect(validateDomainShape("API.Foo.Test")).toBeNull();
  });

  it("rejects the CLI's reject cases", () => {
    expect(validateDomainShape("")).not.toBeNull();
    expect(validateDomainShape("foo")).not.toBeNull(); // no TLD
    expect(validateDomainShape("foo.*.test")).not.toBeNull(); // misplaced wildcard
    expect(validateDomainShape("a_b.test")).not.toBeNull(); // bad char
    expect(validateDomainShape("foo..test")).not.toBeNull(); // empty label
  });

  it("leaves a bare '*' / lone-wildcard for the daemon (shape passes)", () => {
    // `*.test` is shape-valid here but the daemon rejects it (BareWildcard) -
    // this is the intended client-lenient / daemon-authoritative split.
    expect(validateDomainShape("*.test")).toBeNull();
  });
});

describe("isUnderTld", () => {
  it("is true only when the FQDN ends in .<tld>", () => {
    expect(isUnderTld("corp.test", "test")).toBe(true);
    expect(isUnderTld("api.corp.test", "test")).toBe(true);
    expect(isUnderTld("corp.test.", "test")).toBe(true);
    expect(isUnderTld("corp.dev", "test")).toBe(false);
    expect(isUnderTld("test", "test")).toBe(false); // the TLD alone is not under it
  });
});
