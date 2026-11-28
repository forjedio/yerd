import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import InsightsIssueList from "./InsightsIssueList.vue";
import { issuesFixture } from "./fixtures";
import type { IssueChipKey } from "@/lib/insights";

function render(filter: IssueChipKey = "all", limit?: number) {
  return mount(InsightsIssueList, {
    props: { issues: issuesFixture(), busy: null, filter, limit, "onUpdate:filter": () => undefined },
  });
}

describe("InsightsIssueList", () => {
  it("lists the open issues with the open count", () => {
    const w = render();
    expect(w.get('[data-testid="issue-count"]').text()).toBe("5");
    expect(w.findAll("[data-issue]")).toHaveLength(5);
    expect(w.get('[data-issue="0000000000000001"]').text()).toContain("+94 ms");
  });

  it("filters by type and shows dismissed ones under Dismissed", async () => {
    const w = render();
    await w.get('[data-chip="slow_query"]').trigger("click");
    expect(w.emitted("update:filter")?.[0]).toEqual(["slow_query"]);
    await w.setProps({ filter: "slow_query" });
    expect(w.findAll("[data-issue]").map((r) => r.attributes("data-issue"))).toEqual(["0000000000000004"]);
    await w.setProps({ filter: "dismissed" });
    expect(w.findAll("[data-issue]").map((r) => r.attributes("data-issue"))).toEqual([
      "0000000000000006",
      "0000000000000007",
    ]);
    expect(w.get('[data-testid="dismiss"]').attributes("aria-label")).toBe("Restore issue");
  });

  it("emits dismiss and open", async () => {
    const w = render();
    await w.get('[data-issue="0000000000000003"] [data-testid="dismiss"]').trigger("click");
    expect(w.emitted("dismiss")?.[0]).toEqual(["0000000000000003", true]);
    await w.get('[data-issue="0000000000000001"] button').trigger("click");
    expect(w.emitted("open")?.[0]?.[0]).toMatchObject({ key: "0000000000000001" });
  });

  it("keeps a visible focus ring on the issue-opening button", () => {
    const open = render().get('[data-issue="0000000000000001"] button');
    expect(open.classes()).toContain("focus-visible:ring-1");
    expect(open.classes()).toContain("focus-visible:ring-ring");
  });

  it("caps the rows and says how many more", () => {
    const w = render("all", 3);
    expect(w.findAll("[data-issue]")).toHaveLength(3);
    expect(w.text()).toContain("2 more in the Issues tab");
  });
});
