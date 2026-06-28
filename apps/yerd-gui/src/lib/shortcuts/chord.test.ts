import { describe, expect, it } from "vitest";

import { matchChord, type Chord } from "./chord";

interface FakeKey {
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  key?: string;
  code?: string;
}

function ev(e: FakeKey): KeyboardEvent {
  return {
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    key: "",
    code: "",
    ...e,
  } as KeyboardEvent;
}

const CMD_K: Chord = { mod: true, key: "k" };

describe("matchChord - primary accelerator", () => {
  it("resolves mod to Cmd on macOS and Ctrl on Linux", () => {
    expect(matchChord(ev({ metaKey: true, key: "k" }), CMD_K, true)).toBe(true);
    expect(matchChord(ev({ ctrlKey: true, key: "k" }), CMD_K, false)).toBe(true);
  });

  it("does not cross platforms: Ctrl+K is not ⌘K and vice-versa", () => {
    expect(matchChord(ev({ ctrlKey: true, key: "k" }), CMD_K, true)).toBe(false);
    expect(matchChord(ev({ metaKey: true, key: "k" }), CMD_K, false)).toBe(false);
  });

  it("rejects when the secondary modifier is also held", () => {
    expect(
      matchChord(ev({ metaKey: true, ctrlKey: true, key: "k" }), CMD_K, true),
    ).toBe(false);
  });

  it("rejects a bare key with no modifier", () => {
    expect(matchChord(ev({ key: "k" }), CMD_K, true)).toBe(false);
  });
});

describe("matchChord - shift / alt are exact", () => {
  const reverse: Chord = { mod: true, shift: true, key: "r" };
  it("requires shift when specified", () => {
    expect(
      matchChord(ev({ metaKey: true, shiftKey: true, key: "r" }), reverse, true),
    ).toBe(true);
    expect(matchChord(ev({ metaKey: true, key: "r" }), reverse, true)).toBe(false);
  });
  it("rejects an unexpected shift", () => {
    expect(
      matchChord(ev({ metaKey: true, shiftKey: true, key: "k" }), CMD_K, true),
    ).toBe(false);
  });
});

describe("matchChord - code vs key", () => {
  it("matches digits by physical code", () => {
    const chord: Chord = { mod: true, code: "Digit1" };
    expect(matchChord(ev({ metaKey: true, code: "Digit1" }), chord, true)).toBe(true);
    expect(matchChord(ev({ metaKey: true, code: "Digit2" }), chord, true)).toBe(false);
  });
});

describe("matchChord - literal Control (tab cycle)", () => {
  const ctrlTab: Chord = { ctrl: true, code: "Tab" };
  const ctrlShiftTab: Chord = { ctrl: true, shift: true, code: "Tab" };

  it("is Control on both platforms, not Command", () => {
    expect(matchChord(ev({ ctrlKey: true, code: "Tab" }), ctrlTab, true)).toBe(true);
    expect(matchChord(ev({ ctrlKey: true, code: "Tab" }), ctrlTab, false)).toBe(true);
    expect(matchChord(ev({ metaKey: true, code: "Tab" }), ctrlTab, true)).toBe(false);
  });

  it("distinguishes the shifted reverse", () => {
    expect(
      matchChord(ev({ ctrlKey: true, shiftKey: true, code: "Tab" }), ctrlShiftTab, true),
    ).toBe(true);
    expect(matchChord(ev({ ctrlKey: true, code: "Tab" }), ctrlShiftTab, true)).toBe(false);
  });
});
