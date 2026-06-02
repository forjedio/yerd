import { describe, expect, it } from "vitest";

import { formatLoadAvg, humaniseBytes, humaniseUptime } from "./utils";

describe("humaniseBytes", () => {
  it("renders base-2 units", () => {
    expect(humaniseBytes(512)).toBe("512 B");
    expect(humaniseBytes(1536)).toBe("1.5 KB");
    expect(humaniseBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
  it("dashes on null/undefined", () => {
    expect(humaniseBytes(null)).toBe("—");
    expect(humaniseBytes(undefined)).toBe("—");
  });
});

describe("humaniseUptime", () => {
  it("composes d/h/m", () => {
    expect(humaniseUptime(90061)).toBe("1d 1h 1m");
    expect(humaniseUptime(3661)).toBe("1h 1m");
    expect(humaniseUptime(0)).toBe("0s");
  });
});

describe("formatLoadAvg", () => {
  it("converts hundredths back to x.xx (per status.rs)", () => {
    // load_avg is each value × 100; 100 -> 1.00, 50 -> 0.50, 25 -> 0.25.
    expect(formatLoadAvg([100, 50, 25])).toBe("1.00  0.50  0.25");
  });
  it("dashes when unavailable", () => {
    expect(formatLoadAvg(null)).toBe("—");
  });
});
