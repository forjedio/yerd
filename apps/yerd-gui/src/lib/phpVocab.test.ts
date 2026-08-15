import { describe, expect, it } from "vitest";

import { phpVocab } from "./phpVocab";

describe("phpVocab", () => {
  // Pinned as a table, mirroring `crates/yerd-core/src/php_vocab.rs`'s own
  // per-OS test. If the two drift, the wording a user sees stops matching
  // between the GUI and the daemon's own strings.
  it.each([
    [
      false,
      {
        runtime: "PHP-FPM",
        pool: "FPM pool",
        pools: "FPM pools",
        poolShort: "FPM",
        extSuffix: ".so",
        extExample: "/opt/homebrew/lib/php/pecl/20250925/scrypt.so",
      },
    ],
    [
      true,
      {
        runtime: "php-cgi",
        pool: "FastCGI process",
        pools: "FastCGI processes",
        poolShort: "php-cgi",
        extSuffix: ".dll",
        extExample: "C:\\php\\ext\\php_scrypt.dll",
      },
    ],
  ])("returns the full table for isWindows=%s", (isWindows, expected) => {
    expect(phpVocab(isWindows as boolean)).toEqual(expected);
  });

  it("never says FPM on Windows", () => {
    for (const value of Object.values(phpVocab(true))) {
      expect(value).not.toContain("FPM");
    }
  });

  // The plural is a separate field precisely because naive suffixing misspells
  // "processes"; guard that nobody collapses it back.
  it("pluralises without naive suffixing", () => {
    const win = phpVocab(true);
    expect(win.pools).not.toBe(`${win.pool}s`);
    expect(win.pools).toBe("FastCGI processes");
  });

  it("carries the leading dot on the extension suffix", () => {
    expect(phpVocab(true).extSuffix.startsWith(".")).toBe(true);
    expect(phpVocab(false).extSuffix.startsWith(".")).toBe(true);
  });

  // The placeholder is only useful if it is a real path of the host's shape.
  it("gives an example path matching the host's suffix and shape", () => {
    const win = phpVocab(true);
    expect(win.extExample.endsWith(win.extSuffix)).toBe(true);
    expect(win.extExample).toMatch(/^[A-Za-z]:\\/);

    const unix = phpVocab(false);
    expect(unix.extExample.endsWith(unix.extSuffix)).toBe(true);
    expect(unix.extExample.startsWith("/")).toBe(true);
  });
});
