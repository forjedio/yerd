import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import InsightsRequestList from "./InsightsRequestList.vue";
import { requestsFixture } from "./fixtures";

function render(layout: "list" | "table") {
  return mount(InsightsRequestList, {
    props: {
      layout,
      requests: requestsFixture(),
      selected: null,
      loaded: true,
      query: "",
      sort: "slowest" as const,
      "onUpdate:query": () => undefined,
      "onUpdate:sort": () => undefined,
    },
  });
}

describe("InsightsRequestList", () => {
  it("labels a cold request", () => {
    const w = render("list");
    const cold = w.get(`[data-id="${"5".padStart(32, "a")}"]`);
    expect(cold.text()).toContain("cold start, not in stats");
  });

  it("renders a table in the compact layout", () => {
    const w = render("table");
    expect(w.get('[data-testid="insights-requests"]').attributes("data-layout")).toBe("table");
    expect(w.text()).toContain("Queries");
    expect(w.text()).toContain("Dumps");
    expect(w.findAll("[data-id]")).toHaveLength(8);
    expect(w.get(`[data-id="${"5".padStart(32, "a")}"]`).text()).toContain("cold start, not in stats");
  });

  it("emits the picked request", async () => {
    const w = render("list");
    await w.get(`[data-id="${"b".repeat(32)}"]`).trigger("click");
    expect(w.emitted("select")?.[0]?.[0]).toMatchObject({ path: "/orders/1042" });
  });

  it("debounces the filter", async () => {
    vi.useFakeTimers();
    try {
      const w = render("list");
      await w.get('[data-testid="request-filter"]').setValue("orders");
      expect(w.emitted("update:query")).toBeUndefined();
      await vi.advanceTimersByTimeAsync(300);
      expect(w.emitted("update:query")?.[0]).toEqual(["orders"]);
    } finally {
      vi.useRealTimers();
    }
  });
});
