import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const addDomain = vi.fn();
const removeDomain = vi.fn();
const setPrimaryDomain = vi.fn();
const resetDomains = vi.fn();
vi.mock("@/ipc/client", () => ({
  addDomain: (...a: unknown[]) => addDomain(...a),
  removeDomain: (...a: unknown[]) => removeDomain(...a),
  setPrimaryDomain: (...a: unknown[]) => setPrimaryDomain(...a),
  resetDomains: (...a: unknown[]) => resetDomains(...a),
  IpcError: class IpcError extends Error {},
}));

const toastSuccess = vi.fn();
const toastError = vi.fn();
vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import DomainDialog from "./DomainDialog.vue";
import SiteDomainsCard, { type DomainsTarget } from "./SiteDomainsCard.vue";

const custom: DomainsTarget = {
  name: "mydocker",
  primary_domain: "mydocker.test",
  domains: ["mydocker.test", "*.mydocker.test", "docker.test"],
};

function mountCard(site: DomainsTarget = custom, aliasTag?: boolean) {
  return mount(SiteDomainsCard, { props: { site, tld: "test", aliasTag } });
}

function rows(w: ReturnType<typeof mountCard>): string[] {
  return w.findAll('[data-testid="domain-row"]').map((r) => r.text());
}

function control(w: ReturnType<typeof mountCard>, label: string) {
  return w.get(`[aria-label="${label}"]`);
}

beforeEach(() => {
  addDomain.mockReset().mockResolvedValue(undefined);
  removeDomain.mockReset().mockResolvedValue(undefined);
  setPrimaryDomain.mockReset().mockResolvedValue(undefined);
  resetDomains.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SiteDomainsCard rows", () => {
  it("tags a proxy's other exact domains alias when asked", () => {
    expect(rows(mountCard(custom, true))).toEqual([
      "mydocker.testprimary",
      "*.mydocker.testwildcard",
      "docker.testalias",
    ]);
  });

  it("leaves other exact domains untagged by default", () => {
    expect(rows(mountCard())).toEqual(["mydocker.testprimary", "*.mydocker.testwildcard", "docker.test"]);
  });

  it("offers Make primary only on non-primary exact domains", () => {
    const w = mountCard();
    expect(w.find('[aria-label="Make docker.test primary"]').exists()).toBe(true);
    expect(w.find('[aria-label="Make mydocker.test primary"]').exists()).toBe(false);
    expect(w.find('[aria-label="Make *.mydocker.test primary"]').exists()).toBe(false);
  });

  it("synthesizes the apex for a default target, keeps its removal disabled and hides Reset", () => {
    const w = mountCard({ name: "blog" });
    expect(rows(w)).toEqual(["blog.testprimary"]);
    expect(control(w, "Remove blog.test").attributes("disabled")).toBeDefined();
    expect(w.text()).not.toContain("Reset to default");
  });

  it("surfaces an apex-shadowed note", () => {
    const w = mountCard({ name: "blog", apex_shadowed_by: "shop" });
    expect(w.get('[data-testid="domains-shadowed"]').text()).toContain("served by “shop”");
  });
});

describe("SiteDomainsCard actions", () => {
  it("makes a domain primary and asks the parent to reload", async () => {
    const w = mountCard();
    await control(w, "Make docker.test primary").trigger("click");
    await flushPromises();
    expect(setPrimaryDomain).toHaveBeenCalledWith("mydocker", "docker.test");
    expect(w.emitted("domainsChanged")).toHaveLength(1);
  });

  it("removes a domain", async () => {
    const w = mountCard();
    await control(w, "Remove *.mydocker.test").trigger("click");
    await flushPromises();
    expect(removeDomain).toHaveBeenCalledWith("mydocker", "*.mydocker.test");
  });

  it("adds through the dialog and closes it", async () => {
    const w = mountCard();
    await w.findAll("button").find((b) => b.text().includes("Add domain"))!.trigger("click");
    const dialog = w.findComponent(DomainDialog);
    expect(dialog.props("open")).toBe(true);
    expect(dialog.props("editing")).toBeNull();
    dialog.vm.$emit("submit", "api.mydocker.test");
    await flushPromises();
    expect(addDomain).toHaveBeenCalledWith("mydocker", "api.mydocker.test");
    expect(dialog.props("open")).toBe(false);
  });

  it("edits an alias by adding the new domain then removing the old", async () => {
    const w = mountCard();
    await control(w, "Edit docker.test").trigger("click");
    const dialog = w.findComponent(DomainDialog);
    expect(dialog.props("editing")).toBe("docker.test");
    dialog.vm.$emit("submit", "box.test");
    await flushPromises();
    expect(addDomain).toHaveBeenCalledWith("mydocker", "box.test");
    expect(setPrimaryDomain).not.toHaveBeenCalled();
    expect(removeDomain).toHaveBeenCalledWith("mydocker", "docker.test");
  });

  it("moves the primary across when editing the primary", async () => {
    const w = mountCard();
    await control(w, "Edit mydocker.test").trigger("click");
    w.findComponent(DomainDialog).vm.$emit("submit", "main.test");
    await flushPromises();
    expect(addDomain.mock.invocationCallOrder[0]).toBeLessThan(
      setPrimaryDomain.mock.invocationCallOrder[0],
    );
    expect(setPrimaryDomain).toHaveBeenCalledWith("mydocker", "main.test");
    expect(removeDomain).toHaveBeenCalledWith("mydocker", "mydocker.test");
  });

  it("keeps the dialog open and reloads when an edit fails part way", async () => {
    removeDomain.mockRejectedValue(new Error("busy"));
    const w = mountCard();
    await control(w, "Edit docker.test").trigger("click");
    const dialog = w.findComponent(DomainDialog);
    dialog.vm.$emit("submit", "box.test");
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith("Domain change failed", "busy");
    expect(dialog.props("open")).toBe(true);
    expect(w.emitted("domainsChanged")).toHaveLength(1);
  });

  it("resets only after confirming", async () => {
    const w = mountCard();
    await w.findAll("button").find((b) => b.text().includes("Reset to default"))!.trigger("click");
    const confirm = w.findComponent(ConfirmDialog);
    expect(confirm.props("open")).toBe(true);
    expect(resetDomains).not.toHaveBeenCalled();
    confirm.vm.$emit("confirm");
    await flushPromises();
    expect(resetDomains).toHaveBeenCalledWith("mydocker");
  });

  it("locks out a second action while one is in flight", async () => {
    let release!: () => void;
    removeDomain.mockReturnValue(new Promise<void>((r) => (release = r)));
    const w = mountCard();
    await control(w, "Remove *.mydocker.test").trigger("click");
    expect(control(w, "Make docker.test primary").attributes("disabled")).toBeDefined();
    release();
    await flushPromises();
  });
});
