import { mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import DumpsRequestDetail from "./DumpsRequestDetail.vue";
import type { DumpCategory, DumpEvent } from "@/ipc/types";
import { type ChipKey, groupRequests } from "@/lib/dumps";

function ev(id: number, category: DumpCategory, payload: Record<string, unknown> = {}): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000, site: "codestash.test", request_id: "a", payload };
}

const [REQUEST] = groupRequests([
  ev(1, "dump", { value_text: "hello" }),
  ev(2, "cache", { event: "hit", key: "projects" }),
  ev(3, "request", { method: "get", uri: "/projects/42", status: 200, duration_ms: 186.4, ip: "127.0.0.1" }),
]);

const mounted: VueWrapper[] = [];

function mountDetail(props: Record<string, unknown> = {}) {
  const wrapper = mount(DumpsRequestDetail, {
    attachTo: document.body,
    props: {
      request: REQUEST,
      category: "all" as ChipKey,
      query: "",
      "onUpdate:category": (v: ChipKey) => wrapper.setProps({ category: v }),
      "onUpdate:query": (v: string) => wrapper.setProps({ query: v }),
      ...props,
    },
  });
  mounted.push(wrapper);
  return wrapper;
}

function cards(w: VueWrapper): number {
  return w.findAll('[data-testid="dump-event"]').length;
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("DumpsRequestDetail", () => {
  it("draws the header, meta and per-request chip counts", () => {
    const w = mountDetail();
    expect(w.get('[data-testid="detail-title"]').text()).toBe("/projects/42");
    expect(w.get('[data-testid="detail-meta"]').text()).toContain("186.40 ms · 3 events · from 127.0.0.1");
    expect(w.get('[data-chip="all"]').text()).toBe("All 2");
    expect(w.get('[data-chip="cache"]').text()).toBe("Cache 1");
    expect(cards(w)).toBe(2);
    expect(w.find('[aria-label="Close"]').exists()).toBe(false);
  });

  it("shows an empty category with Show all events", async () => {
    const w = mountDetail();
    await w.get('[data-chip="query"]').trigger("click");
    expect(w.get('[data-testid="detail-empty"]').text()).toContain("No queries in this request");
    await w.findAll("button").find((b) => b.text() === "Show all events")?.trigger("click");
    expect(w.get('[data-chip="all"]').attributes("aria-checked")).toBe("true");
    expect(cards(w)).toBe(2);
  });

  it("filters events and collapses the filter on Escape", async () => {
    const w = mountDetail({ closable: true });
    await w.get('[aria-label="Filter events"]').trigger("click");
    const input = w.get('[data-testid="event-filter"]');
    await input.setValue("hello");
    expect(cards(w)).toBe(1);
    await input.trigger("keydown", { key: "Escape" });
    expect(w.find('[data-testid="event-filter"]').exists()).toBe(false);
    expect(cards(w)).toBe(2);
    await w.get('[aria-label="Close"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
  });
});
