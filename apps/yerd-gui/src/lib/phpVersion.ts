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
