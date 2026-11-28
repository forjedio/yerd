import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const openPath = vi.fn((_path: string) => Promise.resolve());
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: (p: string) => openPath(p), revealItemInDir: vi.fn() }));

import InsightsRequestDetail from "./InsightsRequestDetail.vue";
import { detailFixture } from "./fixtures";

beforeEach(() => {
  openPath.mockClear();
});

describe("InsightsRequestDetail", () => {
  it("shows the header, breakdown and issue callout", () => {
    const w = mount(InsightsRequestDetail, { props: { detail: detailFixture() } });
    expect(w.text()).toContain("/orders/1042");
    expect(w.text()).toContain("orders.show · OrderController@show");
    expect(w.get('[data-testid="detail-status"]').text()).toBe("200");
    expect(w.get('[data-testid="detail-time"]').text()).toBe("318 ms");
    expect(w.get('[data-testid="detail-breakdown"]').text()).toContain("Database 201 ms · 52 queries");
    expect(w.get('[data-testid="issue-callout"]').text()).toContain("N+1: the same query ran 48 times");
  });

  it("collapses the N+1 run and expands it on click", async () => {
    const w = mount(InsightsRequestDetail, { props: { detail: detailFixture() } });
    expect(w.findAll('[data-row="group"]')).toHaveLength(1);
    expect(w.get('[data-row="group"]').text()).toContain("48× N+1");
    expect(w.findAll('[data-row="child"]')).toHaveLength(0);
    expect(w.get('[data-row="done"]').text()).toContain("200 OK · 48.2 KB · text/html");
    await w.get('[data-row="group"]').trigger("click");
    expect(w.findAll('[data-row="child"]')).toHaveLength(48);
  });

  it("filters the timeline by chip", async () => {
    const w = mount(InsightsRequestDetail, { props: { detail: detailFixture() } });
    expect(w.text()).toContain("Timeline · 54 events");
    await w.get('[data-chip="http"]').trigger("click");
    expect(w.findAll("[data-row]").map((r) => r.attributes("data-row"))).toEqual(["event"]);
  });

  it("opens the issue's file in the editor", async () => {
    const w = mount(InsightsRequestDetail, { props: { detail: detailFixture() } });
    await w.get('[data-testid="open-in-editor"]').trigger("click");
    await flushPromises();
    expect(openPath).toHaveBeenCalledWith("/Users/me/Code/acme/app/Http/Controllers/OrderController.php");
  });
});
