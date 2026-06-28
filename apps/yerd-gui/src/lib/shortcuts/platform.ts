/**
 * Platform detection for the shortcut layer. Kept out of `chord.ts` so that the
 * matcher stays pure (the boolean is passed in), and cached because the platform
 * doesn't change for the life of the webview.
 */
let cached: boolean | undefined;

/** True on macOS, where the primary accelerator is Command rather than Control. */
export function isMac(): boolean {
  if (cached === undefined) {
    const nav = globalThis.navigator;
    const platform = nav?.platform ?? "";
    const ua = nav?.userAgent ?? "";
    cached = /mac/i.test(platform) || (/mac/i.test(ua) && !/iphone|ipad|ipod/i.test(ua));
  }
  return cached;
}
