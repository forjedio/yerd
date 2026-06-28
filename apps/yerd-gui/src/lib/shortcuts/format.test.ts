import { describe, expect, it } from "vitest";

import { formatChord } from "./format";

describe("formatChord", () => {
  it("renders the primary accelerator per platform", () => {
    expect(formatChord({ mod: true, key: "k" }, true)).toBe("⌘K");
    expect(formatChord({ mod: true, key: "k" }, false)).toBe("Ctrl+K");
  });

  it("orders modifiers and renders the shifted reverse", () => {
    expect(formatChord({ mod: true, shift: true, key: "r" }, true)).toBe("⇧⌘R");
    expect(formatChord({ mod: true, shift: true, key: "r" }, false)).toBe("Ctrl+Shift+R");
  });

  it("labels digit codes", () => {
    expect(formatChord({ mod: true, code: "Digit1" }, true)).toBe("⌘1");
    expect(formatChord({ mod: true, code: "Digit1" }, false)).toBe("Ctrl+1");
  });

  it("labels the tab-cycle chord (literal Control)", () => {
    expect(formatChord({ ctrl: true, code: "Tab" }, true)).toBe("⌃⇥");
    expect(formatChord({ ctrl: true, code: "Tab" }, false)).toBe("Ctrl+Tab");
  });

  it("keeps punctuation keys verbatim", () => {
    expect(formatChord({ mod: true, key: "," }, true)).toBe("⌘,");
    expect(formatChord({ mod: true, key: "/" }, false)).toBe("Ctrl+/");
  });
});
