/**
 * Host-appropriate nouns for the supervised PHP web runtime.
 *
 * Unix serves sites through PHP-FPM, which supervises a pool of workers per
 * version. Windows has no FPM SAPI: the daemon runs `php-cgi.exe` in FastCGI
 * mode, one process per version with no worker pool. Text that says "FPM pool"
 * on Windows is therefore wrong, so components read these nouns instead of
 * spelling them inline.
 *
 * This file is a *type only*. The values come from the daemon host over the
 * `host_platform` command, built from `crates/yerd-core/src/php_vocab.rs`,
 * which is their single definition. Read them via `usePlatform().vocab`.
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
