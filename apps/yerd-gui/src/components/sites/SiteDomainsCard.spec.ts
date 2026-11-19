import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import SiteDomainsCard from "./SiteDomainsCard.vue";

const Pass = { template: "<div><slot /></div>" };

const target = {
  name: "mydocker",
  primary_domain: "mydocker.test",
  domains: ["mydocker.test", "*.mydocker.test", "docker.test"],
};

function mountCard(aliasTag?: boolean) {
  return mount(SiteDomainsCard, {
    attachTo: document.body,
    props: { site: target, tld: "test", aliasTag },
    global: { stubs: { DialogPortal: Pass } },
  });
}

function tags(w: ReturnType<typeof mountCard>): string[] {
  return w
    .get('[data-testid="domains-card"]')
    .findAll("button")
    .slice(0, 3)
    .map((b) => b.text());
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SiteDomainsCard", () => {
  it("tags a proxy's other exact domains alias when asked", () => {
    const w = mountCard(true);
    expect(tags(w)).toEqual(["mydocker.testprimary", "*.mydocker.testwildcard", "docker.testalias"]);
    w.unmount();
  });

  it("leaves other exact domains untagged by default", () => {
    const w = mountCard();
    expect(tags(w)).toEqual(["mydocker.testprimary", "*.mydocker.testwildcard", "docker.test"]);
    w.unmount();
  });

  it("opens the shared panel and forwards its changes", async () => {
    const w = mountCard(true);
    await w.findAll("button").find((b) => b.text().includes("Add domain"))?.trigger("click");
    await flushPromises();
    const panel = w.findComponent({ name: "SiteDomainsPanel" });
    expect(panel.exists()).toBe(true);
    panel.vm.$emit("changed");
    expect(w.emitted("domainsChanged")).toHaveLength(1);
    w.unmount();
  });
});
