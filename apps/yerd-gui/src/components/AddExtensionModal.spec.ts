import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AddExtensionModal from "./AddExtensionModal.vue";

const addPhpExtension = vi.hoisted(() => vi.fn());
const pickExtensionFile = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  addPhpExtension,
  pickExtensionFile,
}));

// The modal's placeholders and success toast follow the host OS, so the
// platform singleton is mocked with a settable value rather than left unloaded.
const hostPlatform = vi.hoisted(() => ({ value: "macos" }));
vi.mock("@/composables/usePlatform", async () => {
  const { computed, ref } = await import("vue");
  const platform = ref(hostPlatform.value);
  return {
    loadPlatform: () => Promise.resolve(),
    usePlatform: () => {
      platform.value = hostPlatform.value;
      return {
        platform,
        isMac: computed(() => platform.value === "macos"),
        isLinux: computed(() => platform.value === "linux"),
        isWindows: computed(() => platform.value === "windows"),
        supportsPathInstall: computed(() => true),
      };
    },
  };
});

const SO = "/opt/homebrew/lib/php/pecl/20250925/scrypt.so";

function mountModal() {
  return mount(AddExtensionModal, {
    props: { open: true, version: "8.4" },
    global: { stubs: { teleport: true } },
  });
}

function byText(w: ReturnType<typeof mountModal>, text: string) {
  return w.findAll("button").find((b) => b.text().includes(text));
}

describe("AddExtensionModal", () => {
  beforeEach(() => {
    addPhpExtension.mockReset();
    pickExtensionFile.mockReset();
    hostPlatform.value = "macos";
  });

  // The field asks the user to type a real path, so a macOS-shaped `.so`
  // example on Windows is actively misleading.
  it("shows an OS-appropriate extension path and suffix", () => {
    const unix = mountModal();
    expect((unix.find("#ext-path").element as HTMLInputElement).placeholder).toBe(SO);
    expect((unix.find("#ext-name").element as HTMLInputElement).placeholder).toContain(".so");

    hostPlatform.value = "windows";
    const win = mountModal();
    const path = (win.find("#ext-path").element as HTMLInputElement).placeholder;
    expect(path).toBe("C:\\php\\ext\\php_scrypt.dll");
    expect(path).not.toContain("/opt/homebrew");
    expect((win.find("#ext-name").element as HTMLInputElement).placeholder).toContain(".dll");
  });

  it("fills the path from the native picker", async () => {
    pickExtensionFile.mockResolvedValue(SO);
    const w = mountModal();

    await byText(w, "Browse…")!.trigger("click");
    await vi.waitFor(() =>
      expect((w.find("#ext-path").element as HTMLInputElement).value).toBe(SO),
    );
  });

  it("leaves the path alone when the picker is cancelled", async () => {
    pickExtensionFile.mockResolvedValue(null);
    const w = mountModal();
    await w.find("#ext-path").setValue(SO);

    await byText(w, "Browse…")!.trigger("click");
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
});
