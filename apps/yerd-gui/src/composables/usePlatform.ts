import { computed, readonly, ref, type ComputedRef, type Ref } from "vue";

import { homeDirectory, hostPlatform } from "@/ipc/client";

// Module-level singleton (mirrors useDaemon/useOnboarding): the host OS is
// fetched once for the whole app, so every view that gates UI on it (Welcome
// journey, Settings → General) agrees instead of each re-probing and
// duplicating isMac/isLinux/supportsPathInstall.
const platform = ref("");
const home = ref<string | null>(null);
let loadPromise: Promise<void> | null = null;
let homePromise: Promise<void> | null = null;

/** Resolve the home directory once. Wrapped so a missing or failing client
 *  call (a test mock, an older host) leaves `home` null instead of throwing. */
function loadHome(): void {
  if (homePromise) return;
  homePromise = Promise.resolve()
    .then(() => homeDirectory())
    .then((dir) => {
      home.value = dir;
    })
    .catch(() => {
      home.value = null;
    });
}

/** Fetch the host platform once; safe to call from multiple components. A
 *  failed call clears the cache so a later call can retry, rather than
 *  leaving `platform` permanently empty. */
export function loadPlatform(): Promise<void> {
  loadHome();
  if (!loadPromise) {
    loadPromise = hostPlatform()
      .then((p) => {
        platform.value = p;
      })
      .catch(() => {
        loadPromise = null;
      });
  }
  return loadPromise;
}

export interface PlatformInfo {
  platform: Readonly<Ref<string>>;
  isMac: ComputedRef<boolean>;
  isLinux: ComputedRef<boolean>;
  /** macOS and Linux only - PATH management isn't wired up for Windows yet. */
  supportsPathInstall: ComputedRef<boolean>;
  /** The user's home directory once resolved, else `null` (show absolute paths). */
  home: Readonly<Ref<string | null>>;
}

export function usePlatform(): PlatformInfo {
  return {
    platform: readonly(platform),
    isMac: computed(() => platform.value === "macos"),
    isLinux: computed(() => platform.value === "linux"),
    supportsPathInstall: computed(() => platform.value === "macos" || platform.value === "linux"),
    home: readonly(home),
  };
}
