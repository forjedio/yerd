import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ShareList from "./ShareList.vue";
import type { ShareItem } from "@/lib/share";

function share(overrides: Partial<ShareItem>): ShareItem {
  return {
    key: "cloudflare:quick:app",
    provider: "cloudflare",
    kind: "quick",
    site: "app",
    staticDomain: false,
    url: "https://fuzzy.trycloudflare.com",
    host: "fuzzy.trycloudflare.com",
    state: "running",
    protected: false,
    lastError: null,
    warnings: [],
    ...overrides,
  };
}

const items: ShareItem[] = [
  share({}),
  share({
    key: "ngrok:quick:app",
    provider: "ngrok",
    staticDomain: true,
    url: "https://demo.ngrok-free.app",
    host: "demo.ngrok-free.app",
  }),
  share({ key: "zrok:quick:blog", provider: "zrok", site: "blog", host: "x.share.zrok.io" }),
];

function mountList(props: Partial<InstanceType<typeof ShareList>["$props"]> = {}) {
  return mount(ShareList, { props: { layout: "list", items, filter: "all", selected: null, ...props } });
}

describe("ShareList", () => {
  it("lists every share with its provider and kind", () => {
    const w = mountList();
    expect(w.get('[data-testid="shares-count"]').text()).toBe("Shares · 3");
    expect(w.findAll("[data-key]").map((r) => r.attributes("data-key"))).toEqual([
      "cloudflare:quick:app",
      "ngrok:quick:app",
      "zrok:quick:blog",
    ]);
    expect(w.findAll('[data-testid="share-kind"]').map((k) => k.text())).toEqual([
      "Cloudflare · Quick link",
      "ngrok · Reserved domain",
      "zrok · Public share",
    ]);
    expect(w.get('[data-testid="share-hint"]').text()).toContain("several providers at once");
  });

  it("filters rows by provider and says when none match", async () => {
    const w = mountList({ filter: "ngrok" });
    expect(w.findAll("[data-key]").map((r) => r.attributes("data-key"))).toEqual(["ngrok:quick:app"]);
    await w.setProps({ items: [items[0] as ShareItem] });
    expect(w.get('[data-testid="share-filter-empty"]').text()).toBe("No ngrok shares.");
  });

  it("emits the filter and the selected key", async () => {
    const w = mountList();
    const zrok = w.findAll('[role="radio"]').find((b) => b.text() === "zrok");
    await zrok?.trigger("click");
    expect(w.emitted("update:filter")?.[0]).toEqual(["zrok"]);
    await w.get('[data-key="ngrok:quick:app"]').trigger("click");
    expect(w.emitted("select")?.[0]).toEqual(["ngrok:quick:app"]);
  });

  it("renders the provider column in the table layout", () => {
    const w = mountList({ layout: "table" });
    expect(w.text()).toContain("Provider");
    expect(w.findAll('[data-testid="share-provider"]').map((c) => c.text())).toEqual([
      "Cloudflare · Quick link",
      "ngrok · Reserved domain",
      "zrok · Public share",
    ]);
  });
});
