import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, type Ref } from "vue";

import { useWideLayout, WIDE_QUERY } from "./useWideLayout";

const original = globalThis.matchMedia;

afterEach(() => {
  globalThis.matchMedia = original;
});

function stubMedia(matches: boolean) {
  const listeners: ((e: MediaQueryListEvent) => void)[] = [];
  const media = {
    matches,
    addEventListener: vi.fn((_: string, fn: (e: MediaQueryListEvent) => void) => listeners.push(fn)),
    removeEventListener: vi.fn(),
  };
  globalThis.matchMedia = vi.fn(() => media) as unknown as typeof globalThis.matchMedia;
  return { media, fire: (m: boolean) => listeners.forEach((fn) => fn({ matches: m } as MediaQueryListEvent)) };
}

function mountProbe() {
  let wide!: Ref<boolean>;
  const w = mount(
    defineComponent({
      setup() {
        wide = useWideLayout();
        return () => h("div");
      },
    }),
  );
  return { w, wide: () => wide.value };
}

describe("useWideLayout", () => {
  it.each([true, false])("starts from the media query (%s)", (matches) => {
    stubMedia(matches);
    const { wide } = mountProbe();
    expect(globalThis.matchMedia).toHaveBeenCalledWith(WIDE_QUERY);
    expect(wide()).toBe(matches);
  });

  it("follows change events and stops listening on unmount", () => {
    const { media, fire } = stubMedia(false);
    const { w, wide } = mountProbe();
    fire(true);
    expect(wide()).toBe(true);
    w.unmount();
    expect(media.removeEventListener).toHaveBeenCalledWith("change", media.addEventListener.mock.calls[0]?.[1]);
  });

  it("is narrow without matchMedia", () => {
    globalThis.matchMedia = undefined as unknown as typeof globalThis.matchMedia;
    expect(mountProbe().wide()).toBe(false);
  });
});
