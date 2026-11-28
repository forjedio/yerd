import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import DumpsRequestList from "./DumpsRequestList.vue";
import type { DumpCategory, DumpEvent, DumpsStatusResponse } from "@/ipc/types";
import { groupRequests } from "@/lib/dumps";

function ev(id: number, request_id: string, category: DumpCategory, payload: Record<string, unknown> = {}): DumpEvent {
  return { id, category, ts_ms: 1_700_000_000_000, site: "codestash.test", request_id, payload };
}

const COUNTS = { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };

function status(enabled: boolean): DumpsStatusResponse {
  return { type: "dumps_status", enabled, port: 2304, running: true, persist: false, extensions: [], counts: COUNTS, features: {} };
}

const REQUESTS = groupRequests([
  ev(1, "a", "query"),
  ev(2, "a", "request", { method: "post", uri: "/projects/42", status: 302, duration_ms: 94 }),
  ev(3, "b", "dump"),
]);

function mountList(props: Record<string, unknown> = {}) {
  return mount(DumpsRequestList, {
    attachTo: document.body,
    props: {
      layout: "list",
      requests: REQUESTS,
      total: REQUESTS.length,
      selected: "a",
      status: status(true),
      busy: null,
      query: "",
      category: "all",
      ...props,
    },
  });
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("DumpsRequestList", () => {
  it("draws summary and summary-less rows, marking the selection in list layout", () => {
    const w = mountList();
    const a = w.get('[data-key="a"]');
    expect(a.attributes("aria-current")).toBe("true");
    expect(a.text()).toContain("POST");
    expect(a.text()).toContain("/projects/42");
    expect(a.text()).toContain("codestash.test · 2 events · 94 ms");
    expect(w.get('[data-key="b"]').text()).toContain("No request summary · 1 event");
    expect(w.find('[data-testid="history-footnote"]').exists()).toBe(true);
  });

  it("draws table rows without a persistent selection", () => {
    const w = mountList({ layout: "table" });
    expect(w.get('[data-key="a"]').attributes("aria-current")).toBeUndefined();
    expect(w.get('[data-key="b"]').text()).toContain("No request summary");
  });

  it("emits Keep history and disables Clear when empty", async () => {
    const w = mountList();
    await w.get('[aria-pressed="false"]').trigger("click");
    expect(w.emitted("togglePersist")?.[0]).toEqual([true]);
    const empty = mountList({ requests: [], total: 0 });
    expect(empty.get('[aria-label="Clear dumps for all sites"]').attributes("disabled")).toBeDefined();
  });

  it("asks before the all-sites clear and emits only on confirm", async () => {
    const w = mountList();
    await w.get('[aria-label="Clear dumps for all sites"]').trigger("click");
    await flushPromises();
    expect(w.emitted("clear")).toBeUndefined();
    expect(document.body.textContent).toContain("Clear dumps for all sites?");
    const confirm = [...document.body.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Clear all sites");
    confirm?.click();
    await flushPromises();
    expect(w.emitted("clear")).toHaveLength(1);
  });

  it.each([
    [true, "Waiting for dumps…"],
    [false, "Interception is paused"],
  ])("empty with interception %s reads %s", async (enabled, copy) => {
    const w = mountList({ requests: [], total: 0, status: status(enabled) });
    expect(w.get('[data-testid="requests-empty"]').text()).toContain(copy);
    if (!enabled) {
      await w.findAll("button").find((b) => b.text() === "Resume")?.trigger("click");
      expect(w.emitted("resume")).toHaveLength(1);
    }
  });

  it("offers Clear filter when the filter hides every request", async () => {
    const w = mountList({ requests: [], query: "nothing" });
    expect(w.text()).toContain("No requests match “nothing”");
    await w.findAll("button").find((b) => b.text() === "Clear filter")?.trigger("click");
    expect(w.emitted("clearFilter")).toHaveLength(1);
  });
});
