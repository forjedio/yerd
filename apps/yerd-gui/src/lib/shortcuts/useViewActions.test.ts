import { afterEach, describe, expect, it, vi } from "vitest";

import { getViewActions, registerViewActions } from "./useViewActions";

afterEach(() => {
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
    disposeA();
    getViewActions().find?.();
    expect(bFind).toHaveBeenCalledOnce();
  });

  it("a reused actions object is not wiped by a stale disposer", () => {
    const shared = { find: vi.fn() };
    const disposeA = registerViewActions(shared);
    registerViewActions(shared); // same object, new active registration
    disposeA();
    expect(getViewActions().find).toBe(shared.find);
  });
});
