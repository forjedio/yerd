import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

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

import ManageDomainsModal from "./ManageDomainsModal.vue";
import type { SiteEntry } from "@/ipc/types";

// Modal teleports to <body>; stub it to render both slots inline so queries work.
const ModalStub = {
  template: '<div><slot /><slot name="footer" :close="() => {}" /></div>',
};

function site(overrides: Partial<SiteEntry> = {}): SiteEntry {
  return {
    name: "blog",
    document_root: "/srv/blog",
    php: "8.3",
    secure: false,
    kind: "linked",
    ...overrides,
  };
}

function mountModal(s: SiteEntry) {
  return mount(ManageDomainsModal, {
    props: { open: true, site: s, tld: "test" },
    global: { stubs: { Modal: ModalStub } },
  });
}

function rows(wrapper: ReturnType<typeof mountModal>) {
  return wrapper.findAll("li");
}

beforeEach(() => {
  addDomain.mockReset().mockResolvedValue(undefined);
  removeDomain.mockReset().mockResolvedValue(undefined);
  setPrimaryDomain.mockReset().mockResolvedValue(undefined);
  resetDomains.mockReset().mockResolvedValue(undefined);
  toastSuccess.mockReset();
  toastError.mockReset();
});

describe("ManageDomainsModal — default site", () => {
  it("synthesizes the apex row, marks it primary, and disables its removal", () => {
    const wrapper = mountModal(site());
    const rs = rows(wrapper);
    expect(rs).toHaveLength(1);
    expect(rs[0].text()).toContain("blog.test");
    expect(rs[0].text()).toContain("primary");

    const removeBtn = wrapper.find('[aria-label="Remove blog.test"]');
    expect((removeBtn.element as HTMLButtonElement).disabled).toBe(true);

    const reset = wrapper.findAll("button").find((b) => b.text().includes("Reset to default"));
    expect((reset!.element as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("ManageDomainsModal — customised site", () => {
  const customised = () =>
    site({
      primary_domain: "corp.test",
      domains: ["blog.test", "corp.test", "*.blog.test"],
    });

  it("badges the primary by value, offers Make primary only on non-primary exacts", () => {
    const wrapper = mountModal(customised());
    const rs = rows(wrapper);
    expect(rs).toHaveLength(3);
    // corp.test is primary (even though it's second in the list).
    const corp = rs.find((r) => r.text().includes("corp.test"))!;
    expect(corp.text()).toContain("primary");
    // blog.test (exact, non-primary) offers Make primary; the wildcard does not.
    const blog = rs.find((r) => r.text().startsWith("blog.test"))!;
    expect(blog.text()).toContain("Make primary");
    const wild = rs.find((r) => r.text().includes("*.blog.test"))!;
    expect(wild.text()).not.toContain("Make primary");
  });

  it("Make primary calls setPrimaryDomain and emits changed", async () => {
    const wrapper = mountModal(customised());
    const blog = rows(wrapper).find((r) => r.text().startsWith("blog.test"))!;
    await blog.find("button").trigger("click");
    await Promise.resolve();
    expect(setPrimaryDomain).toHaveBeenCalledWith("blog", "blog.test");
    expect(wrapper.emitted("changed")).toBeTruthy();
  });

  it("removing an alias calls removeDomain (multiple exacts, so not disabled)", async () => {
    const wrapper = mountModal(customised());
    const removeBtn = wrapper.find('[aria-label="Remove *.blog.test"]');
    expect((removeBtn.element as HTMLButtonElement).disabled).toBe(false);
    await removeBtn.trigger("click");
    await Promise.resolve();
    expect(removeDomain).toHaveBeenCalledWith("blog", "*.blog.test");
  });
});

describe("ManageDomainsModal — add alias", () => {
  it("gates the Add button on shape and sends a valid domain", async () => {
    const wrapper = mountModal(site());
    const input = wrapper.find("#add-domain");
    const addBtn = wrapper.findAll("button").find((b) => b.text().includes("Add"))!;

    // Invalid shape → disabled.
    await input.setValue("nope"); // no TLD
    expect((addBtn.element as HTMLButtonElement).disabled).toBe(true);

    // Valid → enabled → sends.
    await input.setValue("api.blog.test");
    expect((addBtn.element as HTMLButtonElement).disabled).toBe(false);
    await addBtn.trigger("click");
    await Promise.resolve();
    expect(addDomain).toHaveBeenCalledWith("blog", "api.blog.test");
  });

  it("accepts a wildcard alias", async () => {
    const wrapper = mountModal(site());
    const input = wrapper.find("#add-domain");
    const addBtn = wrapper.findAll("button").find((b) => b.text().includes("Add"))!;
    await input.setValue("*.blog.test");
    expect((addBtn.element as HTMLButtonElement).disabled).toBe(false);
    await addBtn.trigger("click");
    await Promise.resolve();
    expect(addDomain).toHaveBeenCalledWith("blog", "*.blog.test");
  });
});

describe("ManageDomainsModal — reset, clear, hints, shadow", () => {
  it("Reset is enabled for a customised site and calls resetDomains", async () => {
    const wrapper = mountModal(site({ primary_domain: "corp.test", domains: ["corp.test"] }));
    const reset = wrapper.findAll("button").find((b) => b.text().includes("Reset to default"))!;
    expect((reset.element as HTMLButtonElement).disabled).toBe(false);
    await reset.trigger("click");
    await Promise.resolve();
    expect(resetDomains).toHaveBeenCalledWith("blog");
    expect(wrapper.emitted("changed")).toBeTruthy();
  });

  it("clears the add field after a successful add", async () => {
    const wrapper = mountModal(site());
    const input = wrapper.find("#add-domain");
    await input.setValue("api.blog.test");
    const addBtn = wrapper.findAll("button").find((b) => b.text().includes("Add"))!;
    await addBtn.trigger("click");
    await Promise.resolve();
    await Promise.resolve();
    expect((input.element as HTMLInputElement).value).toBe("");
  });

  it("shows a non-blocking TLD hint but keeps Add enabled for a wrong-TLD domain", async () => {
    const wrapper = mountModal(site());
    const input = wrapper.find("#add-domain");
    const addBtn = wrapper.findAll("button").find((b) => b.text().includes("Add"))!;
    await input.setValue("corp.dev"); // shape-valid, wrong TLD
    expect((addBtn.element as HTMLButtonElement).disabled).toBe(false);
    expect(wrapper.text()).toContain(".test");
  });

  it("surfaces an apex-shadowed warning", () => {
    const wrapper = mountModal(site({ apex_shadowed_by: "shop" }));
    expect(wrapper.text()).toContain("shop");
    expect(wrapper.text()).toContain("blog.test is currently served by");
  });

  it("locks out a second action while one is in flight", async () => {
    let release!: () => void;
    addDomain.mockReturnValue(new Promise<void>((r) => (release = r)));
    const wrapper = mountModal(
      site({ primary_domain: "corp.test", domains: ["blog.test", "corp.test"] }),
    );
    // Start an add (leaves busy set on the pending promise).
    await wrapper.find("#add-domain").setValue("api.blog.test");
    await wrapper.findAll("button").find((b) => b.text().includes("Add"))!.trigger("click");
    await Promise.resolve();
    // While busy, a Make-primary click must be ignored.
    const blog = rows(wrapper).find((r) => r.text().startsWith("blog.test"))!;
    await blog.find("button").trigger("click");
    expect(setPrimaryDomain).not.toHaveBeenCalled();
    release();
  });
});

describe("ManageDomainsModal — error surfacing", () => {
  it("toasts the daemon message when an action fails", async () => {
    setPrimaryDomain.mockRejectedValue(new Error("already routes to shop"));
    const wrapper = mountModal(
      site({ primary_domain: "corp.test", domains: ["blog.test", "corp.test"] }),
    );
    const blog = rows(wrapper).find((r) => r.text().startsWith("blog.test"))!;
    await blog.find("button").trigger("click");
    await Promise.resolve();
    await Promise.resolve();
    expect(toastError).toHaveBeenCalledWith("Domain change failed", "already routes to shop");
    expect(wrapper.emitted("changed")).toBeFalsy();
  });
});
