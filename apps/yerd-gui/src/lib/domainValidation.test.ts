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

  it("rejects empty, TLD-less, misplaced-wildcard, bad-char, and empty-label inputs", () => {
    expect(validateDomainShape("")).not.toBeNull();
    expect(validateDomainShape("foo")).not.toBeNull();
    expect(validateDomainShape("foo.*.test")).not.toBeNull();
    expect(validateDomainShape("a_b.test")).not.toBeNull();
    expect(validateDomainShape("foo..test")).not.toBeNull();
  });

  it("passes a bare '*.test' through (shape-valid; the daemon rejects it)", () => {
    expect(validateDomainShape("*.test")).toBeNull();
  });
});

describe("isUnderTld", () => {
  it("is true only when the FQDN ends in .<tld>", () => {
    expect(isUnderTld("corp.test", "test")).toBe(true);
    expect(isUnderTld("api.corp.test", "test")).toBe(true);
    expect(isUnderTld("corp.test.", "test")).toBe(true);
    expect(isUnderTld("corp.dev", "test")).toBe(false);
    expect(isUnderTld("test", "test")).toBe(false);
  });
});
