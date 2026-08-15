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

/** Detect the host's editors once; safe to call from multiple components. A
 *  failed call clears the cache so a later call can retry, rather than leaving
 *  `installedIdes` permanently empty. */
export function loadIdes(): Promise<void> {
  if (!loadPromise) {
    const mine = ++generation;
    loadPromise = getInstalledIdes()
      .then((ides) => {
        if (mine === generation) {
          installedIdes.value = ides;
        }
      })
      .catch(() => {
        loadPromise = null;
      });
  }
  return loadPromise;
}

/** Re-run host detection and replace the cached list. Backs the Settings
 *  "Rescan" button, and refreshes the host-side launch-target cache with it. */
export async function rescanIdes(): Promise<void> {
  const mine = ++generation;
  const ides = await getInstalledIdes();
  if (mine === generation) {
    installedIdes.value = ides;
  }
  loadPromise = Promise.resolve();
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
