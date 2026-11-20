import { onUnmounted, ref, type Ref } from "vue";

/** The width at which list screens put the inspector beside the list. */
export const WIDE_QUERY = "(min-width: 1280px)";

/**
 * Whether the window is wide enough for a list screen's list + inspector layout
 * (otherwise it shows a table and opens the inspector as a drawer). Follows the
 * media query's `change` events until the calling component unmounts, and is
 * `false` where `matchMedia` is missing (jsdom).
 */
export function useWideLayout(): Ref<boolean> {
  const media = globalThis.matchMedia?.(WIDE_QUERY) ?? null;
  const wide = ref(media?.matches ?? false);
  function onChange(event: MediaQueryListEvent): void {
    wide.value = event.matches;
  }
  media?.addEventListener?.("change", onChange);
  onUnmounted(() => media?.removeEventListener?.("change", onChange));
  return wide;
}
