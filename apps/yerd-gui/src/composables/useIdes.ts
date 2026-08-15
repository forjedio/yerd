import { computed, ref, type ComputedRef } from "vue";

import { getInstalledIdes } from "@/ipc/client";
import type { IdeOption } from "@/ipc/types";

// Module-level singleton (mirrors usePlatform): host editor detection is a
// filesystem scan, so it runs once per app session and every view that offers an
// editor picker reads the same list instead of re-probing on each open.
const installedIdes = ref<IdeOption[]>([]);
let loadPromise: Promise<void> | null = null;

// Monotonic token identifying the newest detection. A request only publishes its
// result while its captured token is still current, so a slow initial load can't
// overwrite a rescan the user triggered after it (or write into reset state).
let generation = 0;

/** The newest detection, resolving to the count it leaves cached. A superseded
 *  scan chains onto this rather than reading `installedIdes` directly: when the
 *  older scan is the one that resolves first, the cache still holds the
 *  pre-scan list, and reporting that would be a count of neither scan. */
let newest: Promise<number> = Promise.resolve(0);

/**
 * Run one detection under a fresh generation token, publishing its result only
 * while it is still the newest, and resolving to the count that ends up cached
 * - the winner's, for a scan that lost the race.
 *
 * `resetIdes` bumps the generation without starting a scan, so a superseded run
 * can still be `newest` itself; that case reads the cache (which the reset just
 * emptied) instead of awaiting itself forever. A newer run that fails is
 * likewise no reason to fail this one, so its rejection falls back the same way.
 */
function detect(): Promise<number> {
  const mine = ++generation;
  const run: Promise<number> = getInstalledIdes().then((ides) => {
    if (mine === generation) {
      installedIdes.value = ides;
      return ides.length;
    }
    return newest === run
      ? installedIdes.value.length
      : newest.catch(() => installedIdes.value.length);
  });
  newest = run;
  return run;
}

/** Detect the host's editors once; safe to call from multiple components. A
 *  failed call clears the cache so a later call can retry, rather than leaving
 *  `installedIdes` permanently empty. */
export function loadIdes(): Promise<void> {
  if (!loadPromise) {
    loadPromise = detect()
      .then(() => undefined)
      .catch(() => {
        loadPromise = null;
      });
  }
  return loadPromise;
}

/** Re-run host detection and replace the cached list. Backs the Settings
 *  "Rescan" button, and refreshes the host-side launch-target cache with it.
 *  Returns how many editors are now cached, so the button can report the result
 *  back; a superseded scan reports the newer list rather than its own, since
 *  that is what the user is looking at. */
export async function rescanIdes(): Promise<number> {
  const count = await detect();
  loadPromise = Promise.resolve();
  return count;
}

/** Test-only: drop the singleton so each spec starts from a clean detection. */
export function resetIdes(): void {
  generation += 1;
  installedIdes.value = [];
  loadPromise = null;
}

export interface IdesInfo {
  /** Detected editors in host rank order, best first. */
  installedIdes: ComputedRef<IdeOption[]>;
}

export function useIdes(): IdesInfo {
  return { installedIdes: computed(() => installedIdes.value) };
}
