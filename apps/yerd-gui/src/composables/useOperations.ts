import { computed, readonly, ref } from "vue";

/**
 * Singleton registry of long-running operations (daemon start, PHP install,
 * service install, site create, …).
 *
 * The problem this solves: an operation's in-flight UI used to live in the
 * component that kicked it off (a local `busy` ref), so navigating away lost it
 * - the daemon "Start" button reappeared, an install's spinner vanished - even
 * though the work was still running. State here is module-level, like
 * `useDaemon` / `useToast`, so the same operation is visible from every screen
 * and outlives the route change that started it.
 *
 * An operation is keyed by a stable `id` (e.g. `"php-install:8.3"`): `begin`
 * with an existing id replaces it, so a re-issued op can't double-register and a
 * view can read "is this exact thing running?" with `isRunning(id)`.
 */

export type OperationKind =
  | "daemon-start"
  | "daemon-restart"
  | "php-install"
  | "php-update"
  | "service-install"
  | "service-start"
  | "site-create"
  | "tool-install";

export interface Operation {
  /** Stable key; re-`begin`ing with the same id replaces the entry. */
  id: string;
  kind: OperationKind;
  /** Human label for the global indicator, e.g. "Installing PHP 8.3". */
  label: string;
  /** Optional sub-line: the latest streamed log line, or a phase label. */
  detail?: string;
}

// Module-level singleton: one registry for the whole app.
const operations = ref<Operation[]>([]);

function begin(op: Operation): void {
  const rest = operations.value.filter((o) => o.id !== op.id);
  operations.value = [...rest, op];
}

/** Patch a live operation; a no-op if it already ended (avoids reviving it, and
 * avoids needlessly reallocating the array / waking dependents). */
function update(id: string, patch: Partial<Omit<Operation, "id">>): void {
  if (!operations.value.some((o) => o.id === id)) return;
  operations.value = operations.value.map((o) =>
    o.id === id ? { ...o, ...patch } : o,
  );
}

function end(id: string): void {
  operations.value = operations.value.filter((o) => o.id !== id);
}

function isRunning(id: string): boolean {
  return operations.value.some((o) => o.id === id);
}

function get(id: string): Operation | undefined {
  return operations.value.find((o) => o.id === id);
}

const active = computed(() => operations.value);
const any = computed(() => operations.value.length > 0);

export function useOperations() {
  return {
    /** Readonly view of every in-flight operation. */
    active: readonly(active),
    /** True while any operation is running. */
    any,
    begin,
    update,
    end,
    isRunning,
    get,
  };
}
