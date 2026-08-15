/**
 * Host-appropriate nouns for the supervised PHP web runtime.
 *
 * Unix serves sites through PHP-FPM, which supervises a pool of workers per
 * version. Windows has no FPM SAPI: the daemon runs `php-cgi.exe` in FastCGI
 * mode, one process per version with no worker pool. Text that says "FPM pool"
 * on Windows is therefore wrong, so components read these nouns instead of
 * spelling them inline.
 *
 * The GUI learns its OS at runtime from the daemon, so unlike the Rust side
 * this is a function of `isWindows` rather than a compile-time table. It
 * mirrors `crates/yerd-core/src/php_vocab.rs`, whose own table test pins the
 * same values; the two are small enough that duplication beats any codegen
 * bridge.
 *
 * `pools` and `extExample` are GUI-only and have no Rust counterpart: no Rust
 * site needs the plural, and the CLI's `.so`/`.dll` swap is a `cfg_attr` doc
 * literal, which cannot reference a const.
 */
export interface PhpVocab {
  /** The supervised web runtime's name, for prose about the daemon itself. */
  runtime: string;
  /** The per-version serving unit, singular: "restarts only that version's {pool}". */
  pool: string;
  /**
   * The plural of `pool`. Not `pool + "s"`: "FastCGI processes", not "FastCGI
   * processs". Both values start capitalised, so callers can interpolate them
   * sentence-initially without casing them.
   */
  pools: string;
  /** Short form for a table column header or a progress label. */
  poolShort: string;
  /** The host's dynamic-extension file suffix, including the leading dot. */
  extSuffix: string;
  /** A realistic example extension path, for an input placeholder. */
  extExample: string;
}

/**
 * The vocabulary for a host. Pure: callers pass `isWindows` from
 * `usePlatform()` and usually wrap this in a `computed`.
 *
 * With the platform singleton still unloaded, `isWindows` is `false` and Unix
 * wording renders, which is the pre-existing behaviour.
 */
export function phpVocab(isWindows: boolean): PhpVocab {
  return isWindows
    ? {
        runtime: "php-cgi",
        pool: "FastCGI process",
        pools: "FastCGI processes",
        poolShort: "php-cgi",
        extSuffix: ".dll",
        extExample: "C:\\php\\ext\\php_scrypt.dll",
      }
    : {
        runtime: "PHP-FPM",
        pool: "FPM pool",
        pools: "FPM pools",
        poolShort: "FPM",
        extSuffix: ".so",
        extExample: "/opt/homebrew/lib/php/pecl/20250925/scrypt.so",
      };
}
