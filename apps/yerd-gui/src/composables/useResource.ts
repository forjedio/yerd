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
}

const cache = new Map<string, Entry<unknown>>();

function entryFor<T>(key: string, fetcher: () => Promise<T>): Entry<T> {
  const existing = cache.get(key) as Entry<T> | undefined;
  if (existing) {
    // Latest caller wins; callers of the same key pass an identical fetcher.
    existing.fetcher = fetcher;
    return existing;
  }
  const created: Entry<T> = {
    data: shallowRef<T | null>(null),
    error: ref<IpcError | null>(null),
    inFlight: null,
    fetcher,
  };
  cache.set(key, created as Entry<unknown>);
  return created;
}

/** Run the fetch unless one is already in flight (dedup); errors keep last-good data. */
function revalidate<T>(entry: Entry<T>): Promise<void> {
  if (entry.inFlight) return entry.inFlight;
  const run = (async () => {
    try {
      entry.data.value = await entry.fetcher();
      entry.error.value = null;
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
  /** Shared cache value; writing it (or via `mutate`) persists for every reader. */
  data: ShallowRef<T | null>;
  /** True only until the first load settles (no cached value yet) - drives the spinner. */
  loading: Ref<boolean>;
  /** True while a background revalidation runs over already-cached data. */
  refreshing: Ref<boolean>;
  error: Ref<IpcError | null>;
  refresh: () => Promise<void>;
  /** Write the cache directly for optimistic / partial updates. An updater may
   * return `null` (or read a `null` cur) to no-op before the first load. */
  mutate: (next: T | ((cur: T | null) => T | null)) => void;
}

/**
 * Subscribe to the cached resource at `key`, fetching via `fetcher`.
 *
 * `opts.immediate` (default true) controls only THIS call's on-mount fetch, not
 * the shared entry - so a consumer that must not fetch eagerly (e.g. the Overview
 * while the daemon is down) can pass `false` and drive `refresh()` itself, while
 * another consumer of the same key still fetches on mount.
 */
export function useResource<T>(
  key: string,
  fetcher: () => Promise<T>,
  opts: { immediate?: boolean } = {},
): ResourceHandle<T> {
  const { immediate = true } = opts;
  const entry = entryFor<T>(key, fetcher);
  const loading = ref(entry.data.value === null);
  const refreshing = ref(false);

  async function refresh(): Promise<void> {
    const firstLoad = entry.data.value === null;
    if (!firstLoad) refreshing.value = true;
    try {
      await revalidate(entry);
    } finally {
      loading.value = false;
      refreshing.value = false;
    }
  }

  function mutate(next: T | ((cur: T | null) => T | null)): void {
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

/** Silently refetch a key after a mutation; no-op if nothing subscribes to it yet. */
export function invalidate(key: string): Promise<void> {
  const entry = cache.get(key);
  return entry ? revalidate(entry) : Promise.resolve();
}

/** Test-only: drop all cached entries so specs start from a cold cache. */
export function resetResourceCache(): void {
  cache.clear();
}
