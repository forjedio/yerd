import { afterEach, describe, expect, it, vi } from "vitest";

import { getViewActions, registerViewActions } from "./useViewActions";

afterEach(() => {
  // Leave the module-level registry empty between tests.
  registerViewActions({})();
});

describe("registerViewActions", () => {
  it("exposes the latest registration", () => {
    const refresh = vi.fn();
    registerViewActions({ refresh });
    getViewActions().refresh?.();
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("disposer clears the handlers it registered", () => {
    const dispose = registerViewActions({ refresh: vi.fn() });
    dispose();
    expect(getViewActions().refresh).toBeUndefined();
  });

  it("a stale disposer does not wipe a newer view's registration", () => {
    const disposeA = registerViewActions({ find: vi.fn() });
    const bFind = vi.fn();
    registerViewActions({ find: bFind });
    disposeA(); // A unmounts after B mounted - must be a no-op
    getViewActions().find?.();
    expect(bFind).toHaveBeenCalledOnce();
  });
});
