import { describe, expect, it } from "vitest";

import { supportsReliableMaximizedState } from "./windowState";

describe("supportsReliableMaximizedState", () => {
  it("does not persist the unreliable decorationless macOS state", () => {
    expect(supportsReliableMaximizedState("macos")).toBe(false);
  });

  it("supports native maximized state on Linux and Windows", () => {
    expect(supportsReliableMaximizedState("linux")).toBe(true);
    expect(supportsReliableMaximizedState("windows")).toBe(true);
  });

  it("does not persist state for an unknown host", () => {
    expect(supportsReliableMaximizedState("unknown")).toBe(false);
  });
});
