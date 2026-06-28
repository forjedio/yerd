/**
 * Per-view contextual handlers for the shortcuts whose target depends on the
 * active view: Find (⌘F), New (⌘N), Refresh (⌘R), and the dumps-window tab
 * cycle. A view registers its handlers on mount and the returned disposer clears
 * them on unmount; the dispatcher reads the live set when a chord fires.
 *
 * `register` returns a disposer that only clears if its own handlers are still
 * the current ones, so a route change (new view mounts before the old unmounts)
 * never wipes the incoming view's registration.
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

// shallowRef, not ref: a deep reactive proxy would break the identity check in
// the disposer (current.value would be a proxy, never === the raw `actions`).
const current = shallowRef<ViewActions>({});

/** Register the active view's contextual handlers; returns a disposer. */
export function registerViewActions(actions: ViewActions): () => void {
  current.value = actions;
  return () => {
    if (current.value === actions) current.value = {};
  };
}

/** The contextual handlers for the currently mounted view. */
export function getViewActions(): ViewActions {
  return current.value;
}
