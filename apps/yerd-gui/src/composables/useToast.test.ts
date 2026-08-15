import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useToast } from "./useToast";

// The store is a module-level singleton; drain it between cases so one test's
// toasts can't be seen by the next.
afterEach(() => {
  const { toasts, dismiss } = useToast();
  [...toasts.value].forEach((t) => dismiss(t.id));
  vi.useRealTimers();
});

beforeEach(() => {
  vi.useFakeTimers();
});

describe("useToast auto-dismiss", () => {
  const cases = [
    { kind: "success", ttl: 4000 },
    { kind: "info", ttl: 4000 },
    { kind: "error", ttl: 8000 },
  ] as const;

  for (const { kind, ttl } of cases) {
    it(`dismisses a ${kind} toast after ${ttl}ms by default`, () => {
      const toast = useToast();
      toast[kind]("hello");
      expect(toast.toasts.value).toHaveLength(1);

      vi.advanceTimersByTime(ttl - 1);
      expect(toast.toasts.value).toHaveLength(1);

      vi.advanceTimersByTime(1);
      expect(toast.toasts.value).toHaveLength(0);
    });

    it(`honours an explicit ttl on a ${kind} toast`, () => {
      const toast = useToast();
      toast[kind]("hello", undefined, 1500);

      vi.advanceTimersByTime(1499);
      expect(toast.toasts.value).toHaveLength(1);

      vi.advanceTimersByTime(1);
      expect(toast.toasts.value).toHaveLength(0);
    });
  }
});
