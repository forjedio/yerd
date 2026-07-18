import type { PhpVersion } from "@/ipc/types";

/**
 * Compares two major.minor version strings (e.g. "8.3" vs "8.10")
 * numerically, not lexicographically - a plain string compare would put
 * "8.10" before "8.3".
 */
export function comparePhpVersions(a: PhpVersion, b: PhpVersion): number {
  const [aMajor, aMinor] = a.split(".").map(Number);
  const [bMajor, bMinor] = b.split(".").map(Number);
  return aMajor !== bMajor ? aMajor - bMajor : aMinor - bMinor;
}

/** Whether `php` falls within `[minPhp, maxPhp]`, inclusive on both ends. */
export function phpVersionInRange(php: PhpVersion, minPhp: PhpVersion, maxPhp: PhpVersion): boolean {
  return comparePhpVersions(php, minPhp) >= 0 && comparePhpVersions(php, maxPhp) <= 0;
}

/**
 * Whether `php` is an out-of-support legacy version (< 8.2): no coverage, no
 * dumps, and never eligible as the global default. Mirrors the authoritative
 * cutoff in `yerd_core::PhpVersion::is_legacy` (FIRST_SUPPORTED_MINOR = 8.2);
 * the daemon remains the real gate, so this is a UI convenience only.
 */
export function isLegacyVersion(php: PhpVersion): boolean {
  return comparePhpVersions(php, "8.2") < 0;
}
