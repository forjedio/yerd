/**
 * Render a `Chord` as the label users see in the palette and cheat-sheet:
 * macOS glyphs (⌃⌥⇧⌘, joined tight) versus the spelled-out Linux form
 * ("Ctrl+Shift+K"). Order follows each platform's convention.
 */
import type { Chord } from "./chord";

const KEY_LABELS: Record<string, string> = {
  Digit1: "1",
  Digit2: "2",
  Digit3: "3",
  Digit4: "4",
  Digit5: "5",
  Digit6: "6",
  Digit7: "7",
  Digit8: "8",
  Digit9: "9",
};

function keyLabel(chord: Chord, isMac: boolean): string {
  if (chord.code) {
    if (chord.code === "Tab") return isMac ? "⇥" : "Tab";
    const mapped = KEY_LABELS[chord.code];
    if (mapped) return mapped;
    return chord.code;
  }
  const key = chord.key ?? "";
  if (key.length === 1) return key.toUpperCase();
  return key;
}

/** Human-readable shortcut label for `chord`, formatted for the platform. */
export function formatChord(chord: Chord, isMac: boolean): string {
  const key = keyLabel(chord, isMac);
  if (isMac) {
    let out = "";
    if (chord.ctrl) out += "⌃";
    if (chord.alt) out += "⌥";
    if (chord.shift) out += "⇧";
    if (chord.mod) out += "⌘";
    return out + key;
  }
  const parts: string[] = [];
  if (chord.mod || chord.ctrl) parts.push("Ctrl");
  if (chord.alt) parts.push("Alt");
  if (chord.shift) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}
