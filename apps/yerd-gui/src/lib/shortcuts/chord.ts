/**
 * Pure key-chord matching - no DOM listeners, no globals, fully unit-testable.
 *
 * A `Chord` is the platform-independent description of a shortcut. `mod` is the
 * primary accelerator: Command on macOS, Control on Linux (same letter, swapped
 * modifier - the convention this app follows). `ctrl` is the *literal* Control
 * key, used for the few chords that are Control on both platforms (e.g. the
 * dumps-window tab cycle, since Cmd+Tab is the macOS app switcher and can't be
 * repurposed). `shift`/`alt` are literal.
 *
 * Match on `code` for layout-stable physical keys (digits, Tab) and on `key`
 * for character keys (k, comma, slash) so the binding follows the printed glyph.
 */
export interface Chord {
  /** Primary accelerator: Cmd on macOS, Ctrl on Linux. */
  mod?: boolean;
  /** Literal Control key (Control on both platforms). */
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
  /** Character key, matched case-insensitively against `event.key`. */
  key?: string;
  /** Physical key, matched against `event.code` (e.g. "Digit1", "Tab"). */
  code?: string;
}

/** Does this keyboard event satisfy `chord` on the given platform? */
export function matchChord(e: KeyboardEvent, chord: Chord, isMac: boolean): boolean {
  const requiredMeta = isMac ? !!chord.mod : false;
  const requiredCtrl = isMac ? !!chord.ctrl : !!chord.mod || !!chord.ctrl;

  if (e.metaKey !== requiredMeta) return false;
  if (e.ctrlKey !== requiredCtrl) return false;
  if (!!chord.shift !== e.shiftKey) return false;
  if (!!chord.alt !== e.altKey) return false;

  if (chord.code) return e.code === chord.code;
  if (chord.key) return e.key.toLowerCase() === chord.key.toLowerCase();
  return false;
}
