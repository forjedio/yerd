/**
 * Per-view contextual handlers for the shortcuts whose target depends on the
 * active view: Find (⌘F), New (⌘N), Refresh (⌘R), and the dumps-window tab
 * cycle. A view registers its handlers on mount and the returned disposer clears
 * them on unmount; the dispatcher reads the live set when a chord fires.
 *
 * `register` returns a disposer that clears state only if its registration is
 * still the active one (tracked by a per-call token), so a stale disposer from
 * an unmounting view can't wipe a newer view's registration.
 */
import { shallowRef } from "vue";

export interface ViewActions {
  /** Focus this view's text-search field (⌘F). */
  find?: () => void;
  /** Trigger this view's primary create/add action (⌘N). */
  create?: () => void;
  /** Refetch this view's data (⌘R). */
  refresh?: () => void;
  /** Dumps window: select the previous category tab. */
  prevTab?: () => void;
  /** Dumps window: select the next category tab. */
  nextTab?: () => void;
}

// shallowRef: the handlers are invoked imperatively by the dispatcher, never
// rendered, so deep reactivity would be wasted work.
const current = shallowRef<ViewActions>({});
// Monotonic registration id: a disposer clears state only while its own
// registration is still the active one, so a stale disposer can't wipe a newer
// view's registration even when the same actions object is reused.
let activeToken = 0;

/** Register the active view's contextual handlers; returns a disposer. */
export function registerViewActions(actions: ViewActions): () => void {
  const token = ++activeToken;
  current.value = actions;
  return () => {
    if (token === activeToken) current.value = {};
  };
}

/** The contextual handlers for the currently mounted view. */
export function getViewActions(): ViewActions {
  return current.value;
}
