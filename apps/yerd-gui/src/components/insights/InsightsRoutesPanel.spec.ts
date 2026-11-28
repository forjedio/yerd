import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import InsightsRoutesPanel from "./InsightsRoutesPanel.vue";
import { overviewFixture, requestsFixture } from "./fixtures";

function render(tab: "routes" | "requests" | "queries") {
  const o = overviewFixture();
  return mount(InsightsRoutesPanel, {
    props: { routes: o.routes, queries: o.queries, slowest: requestsFixture(), tab, "onUpdate:tab": () => undefined },
  });
}

describe("InsightsRoutesPanel", () => {
  it("lists routes by default", () => {
    const w = render("routes");
    expect(w.findAll('[data-testid="route-row"]')).toHaveLength(4);
    expect(w.text()).toContain("Sorted by total time spent");
  });

  it("switches sub-tabs through the model", async () => {
    const w = render("routes");
    await w.get('[data-tab="requests"]').trigger("click");
    expect(w.emitted("update:tab")?.[0]).toEqual(["requests"]);
  });

  it("opens a slow request", async () => {
    const w = render("requests");
    await w.findAll('[data-testid="slow-row"]')[1]?.trigger("click");
    expect(w.emitted("open")?.[0]?.[0]).toMatchObject({ path: "/orders/1042" });
  });

  it("lists query fingerprints", () => {
    const w = render("queries");
    expect(w.findAll('[data-testid="query-row"]')).toHaveLength(3);
  });
});
