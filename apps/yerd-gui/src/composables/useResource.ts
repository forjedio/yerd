import { onMounted, ref, shallowRef, type Ref, type ShallowRef } from "vue";

import { IpcError } from "@/ipc/client";

/**
 * Stale-while-revalidate cache for daemon-backed view data.
 *
 * Views used to fetch in `onMounted` with a local `loading` ref and no cache, so
 * every revisit re-paid the round-trip and flashed an empty/spinner state even
 * when the data was seconds-stale. `useResource` keeps one shared value per key
 * at module scope (like `useDaemon` / `useToast`): a revisit renders the cached
 * value instantly and revalidates silently underneath, so the full-page spinner
 * shows only on the genuine first load.
 *
 * The cache entry (and its value ref) is module-lived - it is never torn down on
 * unmount, so two simultaneous consumers of the same key (e.g. the always-mounted
 * command palette and the active view both reading `"sites"`) share one value and
 * one in-flight fetch.
 */

interface Entry<T> {
  data: ShallowRef<T | null>;
  error: Ref<IpcError | null>;
  /** Non-null while a fetch is in flight; shared so concurrent callers dedup. */
  inFlight: Promise<void> | null;
  fetcher: () => Promise<T>;
  /** Bumped on every `mutate`; a fetch that started before the bump must not
   * overwrite the newer optimistic value when it finally resolves. */
  epoch: number;
}

const cache = new Map<string, Entry<unknown>>();

/**
 * The shared entry for `key`, created on first use. Re-supplying the fetcher is a
 * latest-caller-wins no-op: every consumer of a given key is expected to pass an
 * identical fetcher, so the shared dedup/cache stay coherent.
 */
function entryFor<T>(key: string, fetcher: () => Promise<T>): Entry<T> {
  const existing = cache.get(key) as Entry<T> | undefined;
  if (existing) {
    existing.fetcher = fetcher;
    return existing;
  }
  const created: Entry<T> = {
    data: shallowRef<T | null>(null),
    error: ref<IpcError | null>(null),
    inFlight: null,
    fetcher,
    epoch: 0,
  };
  cache.set(key, created as Entry<unknown>);
  return created;
}

/**
 * Run the fetch unless one is already in flight (dedup); errors keep last-good
 * data. The captured `epoch` is the optimistic-write guard: if a `mutate` landed
 * during the fetch the epoch advances, and this now-stale response is discarded
 * rather than clobbering the newer value.
 *
 * `force` is for a post-write refresh: an in-flight fetch may have started before
 * the write reached the daemon, so deduping onto it would observe pre-write state.
 * A forced call instead chains a fresh fetch after the in-flight one settles, so
 * the caller is guaranteed to see post-write data.
 */
function revalidate<T>(entry: Entry<T>, force = false): Promise<void> {
  if (entry.inFlight) {
    return force ? entry.inFlight.then(() => revalidate(entry, false)) : entry.inFlight;
  }
  const epoch = entry.epoch;
  const run = (async () => {
    try {
      const value = await entry.fetcher();
      if (entry.epoch === epoch) {
        entry.data.value = value;
        entry.error.value = null;
      }
    } catch (e) {
      entry.error.value = e instanceof IpcError ? e : new IpcError(String(e));
    } finally {
      entry.inFlight = null;
    }
  })();
  entry.inFlight = run;
  return run;
}

export interface ResourceHandle<T> {
  /** Shared cache value (read-only); use `mutate` to write so the epoch guard
   * applies and a stale in-flight fetch can't clobber an optimistic update. */
  data: Readonly<ShallowRef<T | null>>;
  /** True only until the first load settles (no cached value yet) - drives the spinner. */
  loading: Ref<boolean>;
  /** True while a background revalidation runs over already-cached data. */
  refreshing: Ref<boolean>;
  error: Ref<IpcError | null>;
  /** Revalidate the shared cache. Pass `{ force: true }` after an awaited write so
   * a fetch already in flight (possibly pre-write) can't satisfy this refresh. */
  refresh: (opts?: { force?: boolean }) => Promise<void>;
  /** Write the cache directly for optimistic / partial updates. An updater may
   * return `null` to leave/clear the value before the first load. */
  mutate: (next: T | ((cur: T | null) => T | null)) => void;
}

/**
 * Subscribe to the cached resource at `key`, fetching via `fetcher`.
 *
 * `opts.immediate` (default true) controls only THIS call's on-mount fetch, not
 * the shared entry - so a consumer that must not fetch eagerly (e.g. the Overview
 * while the daemon is down) can pass `false` and drive `refresh()` itself, while
 * another consumer of the same key still fetches on mount.
 *
 * `loading` starts true only for an eager consumer over a cold cache (an
 * `immediate:false` consumer never auto-fetches, so it must not spin until it
 * calls `refresh()`). Each `refresh` then drives `loading` on a first load
 * (including a retry after a failed first load) and `refreshing` on a
 * revalidation over already-cached data.
 */
export function useResource<T>(
  key: string,
  fetcher: () => Promise<T>,
  opts: { immediate?: boolean } = {},
): ResourceHandle<T> {
  const { immediate = true } = opts;
  const entry = entryFor<T>(key, fetcher);
  const loading = ref(immediate && entry.data.value === null);
  const refreshing = ref(false);

  async function refresh(opts: { force?: boolean } = {}): Promise<void> {
    const firstLoad = entry.data.value === null;
    if (firstLoad) loading.value = true;
    else refreshing.value = true;
    try {
      await revalidate(entry, opts.force);
    } finally {
      loading.value = false;
      refreshing.value = false;
    }
  }

  function mutate(next: T | ((cur: T | null) => T | null)): void {
    entry.epoch += 1;
    entry.data.value =
      typeof next === "function" ? (next as (cur: T | null) => T | null)(entry.data.value) : next;
  }

  if (immediate) {
    onMounted(() => {
      void refresh();
    });
  }

  return { data: entry.data, loading, refreshing, error: entry.error, refresh, mutate };
}

/** Silently refetch a key after a mutation; no-op if nothing subscribes to it yet.
 * Forced, since it follows a write: it must not dedupe onto a pre-write fetch. */
export function invalidate(key: string): Promise<void> {
  const entry = cache.get(key);
  return entry ? revalidate(entry, true) : Promise.resolve();
}

/** Test-only: drop all cached entries so specs start from a cold cache. */
export function resetResourceCache(): void {
  cache.clear();
}
