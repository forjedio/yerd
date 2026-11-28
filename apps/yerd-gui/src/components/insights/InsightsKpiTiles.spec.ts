import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import InsightsKpiTiles from "./InsightsKpiTiles.vue";
import { overviewFixture } from "./fixtures";

describe("InsightsKpiTiles", () => {
  it("renders the five tiles from an overview", () => {
    const w = mount(InsightsKpiTiles, { props: { overview: overviewFixture() } });
    const text = (k: string) => w.get(`[data-kpi="${k}"]`).text();
    expect(text("p50")).toContain("68");
    expect(text("p50")).toContain("9 ms vs previous hour");
    expect(text("p95")).toContain("231");
    expect(text("requests")).toContain("1,846");
    expect(text("requests")).toContain("31/min · 412 static skipped");
    expect(text("failed")).toContain("1.9");
    expect(text("failed")).toContain("29 × 5xx · 6 × 4xx");
    expect(text("queries")).toContain("Queries / request");
    expect(text("queries")).toContain("14avg");
    expect(text("queries")).toContain("max 212 on GET /orders/{order}");
  });

  it("shows the cold and clean-recording status line", () => {
    const w = mount(InsightsKpiTiles, { props: { overview: overviewFixture() } });
    const line = w.get('[data-testid="status-line"]').text();
    expect(line).toContain("4 cold requests set aside");
    expect(line).toContain("Every request recorded, 0 samples dropped");
  });

  it("reports dropped samples", () => {
    const o = { ...overviewFixture(), cold: 0, dropped_requests: 2, dropped_frames: 5 };
    const w = mount(InsightsKpiTiles, { props: { overview: o } });
    expect(w.get('[data-testid="status-line"]').text()).toBe("2 requests, 5 frames dropped");
  });
});
