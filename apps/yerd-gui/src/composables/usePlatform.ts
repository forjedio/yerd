import { computed, readonly, ref, type ComputedRef, type Ref } from "vue";

import { hostPlatform } from "@/ipc/client";
import type { PhpVocab } from "@/lib/phpVocab";

// Module-level singleton (mirrors useDaemon/useOnboarding): the host OS is
// fetched once for the whole app, so every view that gates UI on it (Welcome
// journey, Settings → General) agrees instead of each re-probing and
// duplicating isMac/isLinux/supportsPathInstall.
const platform = ref("");

// The pre-load default only. `hostPlatform()` is an `invoke`, so it resolves
// after the first render; seeding the Unix row reproduces the previous
// behaviour exactly, where `platform` started `""` and the Unix wording
// rendered. The daemon host is the authority and overwrites this as soon as it
// answers - never treat these values as the definition, which lives in
// `crates/yerd-core/src/php_vocab.rs`.
const vocab = ref<PhpVocab>({
  runtime: "PHP-FPM",
  pool: "FPM pool",
  pools: "FPM pools",
  poolShort: "FPM",
  extSuffix: ".so",
  extExample: "/opt/homebrew/lib/php/pecl/20250925/scrypt.so",
});
let loadPromise: Promise<void> | null = null;

/** Fetch the host platform once; safe to call from multiple components. A
 *  failed call clears the cache so a later call can retry, rather than
 *  leaving `platform` permanently empty. */
export function loadPlatform(): Promise<void> {
  if (!loadPromise) {
    loadPromise = hostPlatform()
      .then((p) => {
        platform.value = p.os;
        vocab.value = p.vocab;
      })
      .catch(() => {
        loadPromise = null;
      });
  }
  return loadPromise;
}

export interface PlatformInfo {
  platform: Readonly<Ref<string>>;
  /** Host-appropriate PHP nouns, delivered by the daemon host. */
  vocab: Readonly<Ref<PhpVocab>>;
  isMac: ComputedRef<boolean>;
  isLinux: ComputedRef<boolean>;
  isWindows: ComputedRef<boolean>;
  /** macOS, Linux, and Windows: each wires `yerd path install` (a copy/symlink
   *  plus a PATH edit) through the CLI. */
  supportsPathInstall: ComputedRef<boolean>;
}

export function usePlatform(): PlatformInfo {
  return {
    platform: readonly(platform),
    vocab: readonly(vocab) as Readonly<Ref<PhpVocab>>,
    isMac: computed(() => platform.value === "macos"),
    isLinux: computed(() => platform.value === "linux"),
    isWindows: computed(() => platform.value === "windows"),
    supportsPathInstall: computed(
      () =>
        platform.value === "macos" || platform.value === "linux" || platform.value === "windows",
    ),
  };
}
