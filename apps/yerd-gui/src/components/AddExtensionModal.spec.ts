import { mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AddExtensionModal from "./AddExtensionModal.vue";

const addPhpExtension = vi.hoisted(() => vi.fn());
const pickExtensionFile = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  addPhpExtension,
  pickExtensionFile,
}));

const SO = "/opt/homebrew/lib/php/pecl/20250925/scrypt.so";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

function mountModal() {
  const w = mount(AddExtensionModal, {
    attachTo: document.body,
    props: { open: true, version: "8.4" },
    global: { stubs: { DialogPortal: Pass } },
  });
  mounted.push(w);
  return w;
}

function byText(w: ReturnType<typeof mountModal>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

describe("AddExtensionModal", () => {
  beforeEach(() => {
    addPhpExtension.mockReset();
    pickExtensionFile.mockReset();
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
    document.body.innerHTML = "";
  });

  it("titles the dialog with the version", () => {
    const w = mountModal();
    expect(w.text()).toContain("Add an extension to PHP 8.4");
  });

  it("fills the path from the native picker", async () => {
    pickExtensionFile.mockResolvedValue(SO);
    const w = mountModal();

    await byText(w, "Choose")!.trigger("click");
    await vi.waitFor(() =>
      expect((w.find("#ext-path").element as HTMLInputElement).value).toBe(SO),
    );
  });

  it("leaves the path alone when the picker is cancelled", async () => {
    pickExtensionFile.mockResolvedValue(null);
    const w = mountModal();
    await w.find("#ext-path").setValue(SO);

    await byText(w, "Choose")!.trigger("click");
    await vi.waitFor(() => expect(pickExtensionFile).toHaveBeenCalled());
    expect((w.find("#ext-path").element as HTMLInputElement).value).toBe(SO);
  });

  it("registers the extension against the given version and emits the new map", async () => {
    const map = { "8.4": [{ name: "scrypt", path: SO, zend: false, present: true }] };
    addPhpExtension.mockResolvedValue(map);
    const w = mountModal();

    await w.find("#ext-path").setValue(SO);
    await byText(w, "Add extension")!.trigger("click");
    await vi.waitFor(() => expect(addPhpExtension).toHaveBeenCalled());

    expect(addPhpExtension).toHaveBeenCalledWith("8.4", SO, false, undefined);
    expect(w.emitted("added")?.[0]).toEqual([map]);
    const opens = w.emitted("update:open");
    expect(opens?.[opens.length - 1]).toEqual([false]);
  });

  it("passes an explicit name and the zend flag through", async () => {
    addPhpExtension.mockResolvedValue({});
    const w = mountModal();

    await w.find("#ext-path").setValue(SO);
    await w.find("#ext-name").setValue("scrypt2");
    await w.find('button[aria-label="Load as a Zend extension"]').trigger("click");
    await byText(w, "Add extension")!.trigger("click");
    await vi.waitFor(() => expect(addPhpExtension).toHaveBeenCalled());

    expect(addPhpExtension).toHaveBeenCalledWith("8.4", SO, true, "scrypt2");
  });

  it("shows a failed load probe inline and stays open with the path intact", async () => {
    addPhpExtension.mockRejectedValue(
      new Error("extension failed to load into PHP 8.4: undefined symbol"),
    );
    const w = mountModal();

    await w.find("#ext-path").setValue(SO);
    await byText(w, "Add extension")!.trigger("click");
    await vi.waitFor(() => expect(w.text()).toContain("undefined symbol"));

    expect(w.emitted("update:open")).toBeUndefined();
    expect((w.find("#ext-path").element as HTMLInputElement).value).toBe(SO);
  });

  it("refuses to submit without a path", async () => {
    const w = mountModal();
    await byText(w, "Add extension")!.trigger("click");

    expect(addPhpExtension).not.toHaveBeenCalled();
    expect(w.text()).toContain("choose an extension file");
  });

  it("clears its fields when reopened", async () => {
    const w = mountModal();
    await w.find("#ext-path").setValue(SO);

    await w.setProps({ open: false });
    await w.setProps({ open: true });

    expect((w.find("#ext-path").element as HTMLInputElement).value).toBe("");
  });

  it("keeps a stale add from closing or erroring into the dialog reopened for another version", async () => {
    let fail: (e: Error) => void = () => {};
    addPhpExtension.mockReturnValueOnce(new Promise((_, reject) => (fail = reject)));
    const w = mountModal();
    await w.find("#ext-path").setValue(SO);
    await byText(w, "Add extension")!.trigger("click");
    expect(byText(w, "Checking")).toBeDefined();

    await w.setProps({ open: false });
    await w.setProps({ open: true, version: "8.3" });
    expect(byText(w, "Add extension")!.attributes("disabled")).toBeUndefined();

    fail(new Error("undefined symbol"));
    await vi.waitFor(() => expect(addPhpExtension).toHaveBeenCalledTimes(1));
    await new Promise((r) => setTimeout(r, 0));
    expect(w.text()).not.toContain("undefined symbol");
    expect(w.emitted("update:open")).toBeUndefined();
    expect(byText(w, "Add extension")!.attributes("disabled")).toBeUndefined();
  });

  it("still reports a stale success to the view without closing the new dialog", async () => {
    let succeed: (m: unknown) => void = () => {};
    addPhpExtension.mockReturnValueOnce(new Promise((resolve) => (succeed = resolve)));
    const w = mountModal();
    await w.find("#ext-path").setValue(SO);
    await byText(w, "Add extension")!.trigger("click");
    await w.setProps({ version: "8.3" });

    succeed({ "8.4": [] });
    await vi.waitFor(() => expect(w.emitted("added")).toEqual([[{ "8.4": [] }]]));
    expect(w.emitted("update:open")).toBeUndefined();
  });
});
