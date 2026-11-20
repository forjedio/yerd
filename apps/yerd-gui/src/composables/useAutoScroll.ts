import { ref, watch, type Ref } from "vue";

/**
 * A template ref for a streamed log box that stays pinned to its bottom: each
 * time `length` changes the box scrolls to its end, after the DOM has updated.
 * Bind the returned ref to the scrolling element.
 */
export function useAutoScroll(length: () => number): Ref<HTMLElement | null> {
  const box = ref<HTMLElement | null>(null);
  watch(
    length,
    () => {
      const el = box.value;
      if (el) el.scrollTop = el.scrollHeight;
    },
    { flush: "post" },
  );
  return box;
}
