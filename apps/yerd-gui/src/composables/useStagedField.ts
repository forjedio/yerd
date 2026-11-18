import { computed, ref, type ComputedRef, type Ref } from "vue";

/** Field values that compare correctly by identity. */
export type StagedScalar = string | number | boolean | bigint | symbol | null | undefined;

/** One staged form field reconciled against the daemon's latest saved value. */
export interface StagedField<T> {
  /** What the control shows; the user's edit while dirty, else the saved value. */
  draft: Readonly<Ref<T>>;
  /** True while the draft differs from the latest saved value. */
  dirty: ComputedRef<boolean>;
  /** Record the user's edit. Reverting to the value the edit started from snaps to the latest saved value. */
  edit(value: T): void;
  /** Take a new saved value from the daemon, keeping an in-progress edit. */
  sync(saved: T): void;
  /** Take a new saved value and drop any edit (after a successful save, or Discard). */
  reset(saved: T): void;
}

/**
 * A field staged behind an explicit save, where daemon refreshes can land while
 * the user is editing. Each field reconciles on its own, so a refresh updates
 * every untouched field even while another one holds an edit.
 *
 * Three values are tracked: the latest `saved` value, the `base` the current
 * edit started from, and the `draft`. Reverting an edit back to `base` shows
 * `saved` rather than the stale base, so an out-of-band change made while the
 * user was editing is not hidden.
 *
 * Scalar fields compare by identity (`Object.is`). Any other field type must
 * pass a `key` that maps equal values to the same string, since identity would
 * call every fresh array or object a change.
 */
export function useStagedField<T extends StagedScalar>(initial: T): StagedField<T>;
export function useStagedField<T>(initial: T, key: (v: T) => string): StagedField<T>;
export function useStagedField<T>(initial: T, key?: (v: T) => string): StagedField<T> {
  const saved = ref(initial) as Ref<T>;
  const base = ref(initial) as Ref<T>;
  const draft = ref(initial) as Ref<T>;

  const same = (a: T, b: T): boolean => (key ? key(a) === key(b) : Object.is(a, b));
  const dirty = computed(() => !same(draft.value, saved.value));

  function settle(): void {
    if (!dirty.value) base.value = saved.value;
  }

  function edit(value: T): void {
    draft.value = same(value, base.value) ? saved.value : value;
    settle();
  }

  function sync(next: T): void {
    if (!dirty.value) draft.value = next;
    saved.value = next;
    settle();
  }

  function reset(next: T): void {
    saved.value = next;
    base.value = next;
    draft.value = next;
  }

  return { draft, dirty, edit, sync, reset };
}
