import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const pickDirectory = vi.fn();
const projectConfigDraft = vi.fn();
const suggestSiteName = vi.fn();
vi.mock("@/ipc/client", () => {
  class IpcError extends Error {
    readonly code: string;
    constructor(message: string, code = "internal") {
      super(message);
      this.code = code;
    }
  }
  return {
    IpcError,
    pickDirectory: (...args: unknown[]) => pickDirectory(...args),
    projectConfigDraft: (...args: unknown[]) => projectConfigDraft(...args),
    suggestSiteName: (...args: unknown[]) => suggestSiteName(...args),
  };
});

import { IpcError } from "@/ipc/client";
import type { ProjectConfig } from "@/ipc/types";
import { slugifySiteName } from "@/lib/siteName";

import LinkFolderDialog from "./LinkFolderDialog.vue";

function mountDialog(props: Record<string, unknown> = {}) {
  return mount(LinkFolderDialog, {
    props: {
      open: true,
      tld: "test",
      home: "/Users/a",
      phpVersions: ["8.4", "8.3"],
      defaultPhp: "8.3",
      ...props,
    },
    global: { stubs: { teleport: true, DialogPortal: { template: "<div><slot /></div>" } } },
  });
}

type Dialog = ReturnType<typeof mountDialog>;

function button(w: Dialog, text: string) {
  const b = w.findAll("button").find((x) => x.text().includes(text));
  if (!b) throw new Error(`no "${text}" button`);
  return b;
}

function nameInput(w: Dialog) {
  return w.get('[aria-label="Site name"]');
}

function pathInput(w: Dialog) {
  return w.get('[aria-label="Project folder"]');
}

function draft(overrides: Partial<ProjectConfig> = {}): ProjectConfig {
  return {
    file: "/srv/app/.yerd.toml",
    exists: false,
    site: null,
    name: "app",
    contents: "",
    ...overrides,
  };
}

function deferredDraft(): { resolve: (value: ProjectConfig) => void } {
  const out = { resolve: (_value: ProjectConfig) => undefined as void };
  projectConfigDraft.mockReturnValue(
    new Promise<ProjectConfig>((resolve) => {
      out.resolve = resolve;
    }),
  );
  return out;
}

beforeEach(() => {
  pickDirectory.mockReset();
  projectConfigDraft.mockReset().mockResolvedValue(draft());
  suggestSiteName.mockReset().mockImplementation((folder: string) => Promise.resolve(slugifySiteName(folder)));
});

describe("LinkFolderDialog", () => {
  it("prefills the name from the chosen folder and shows it relative to home", async () => {
    pickDirectory.mockResolvedValue("/Users/a/Code/Billing_API");
    const w = mountDialog();
    expect((pathInput(w).element as HTMLInputElement).value).toBe("");
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect((pathInput(w).element as HTMLInputElement).value).toBe("~/Code/Billing_API");
    expect((nameInput(w).element as HTMLInputElement).value).toBe("billing-api");
    expect(button(w, "Change").exists()).toBe(true);
  });

  it("prefills the suffix-stripped name the bridge suggests", async () => {
    pickDirectory.mockResolvedValue("/srv/laravel.com");
    suggestSiteName.mockResolvedValue("laravel");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(suggestSiteName).toHaveBeenCalledWith("laravel.com");
    expect((nameInput(w).element as HTMLInputElement).value).toBe("laravel");
  });

  it("fills the suggested name before a slow draft lookup finishes", async () => {
    const pending = deferredDraft();
    pickDirectory.mockResolvedValue("/srv/laravel.com");
    suggestSiteName.mockResolvedValue("laravel");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect((nameInput(w).element as HTMLInputElement).value).toBe("laravel");
    expect(button(w, "Link site").attributes("disabled")).toBeDefined();
    pending.resolve(draft());
    await flushPromises();
    expect(button(w, "Link site").attributes("disabled")).toBeUndefined();
  });

  it("applies a nameless .yerd.toml under the suggested name", async () => {
    pickDirectory.mockResolvedValue("/srv/laravel.com");
    suggestSiteName.mockResolvedValue("laravel");
    projectConfigDraft.mockResolvedValue(draft({ exists: true, name: null }));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="project-file-name"]').text()).toBe("laravel.test");
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toMatchObject({ name: "laravel", applyProjectFile: true });
  });

  it("does not overwrite a typed name with the suggestion", async () => {
    pickDirectory.mockResolvedValue("/srv/laravel.com");
    suggestSiteName.mockResolvedValue("laravel");
    const w = mountDialog();
    await nameInput(w).setValue("mine");
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect((nameInput(w).element as HTMLInputElement).value).toBe("mine");
  });

  it("still links under the suggested name when the draft lookup fails", async () => {
    pickDirectory.mockResolvedValue("/srv/laravel.com");
    suggestSiteName.mockResolvedValue("laravel");
    projectConfigDraft.mockRejectedValue(new IpcError("no such folder", "invalid_path"));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toMatchObject({
      name: "laravel",
      path: "/srv/laravel.com",
      applyProjectFile: false,
    });
  });

  it("falls back to the plain slug when the suggestion fails", async () => {
    pickDirectory.mockResolvedValue("/srv/My_App");
    suggestSiteName.mockRejectedValue(new Error("bridge down"));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect((nameInput(w).element as HTMLInputElement).value).toBe("my-app");
  });

  it("keeps a name the user typed when the folder changes", async () => {
    pickDirectory.mockResolvedValue("/srv/other");
    const w = mountDialog();
    await nameInput(w).setValue("mine");
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect((nameInput(w).element as HTMLInputElement).value).toBe("mine");
  });

  it("enables Link only with a folder and a name that slugifies", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    const w = mountDialog();
    expect(button(w, "Link site").attributes("disabled")).toBeDefined();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(button(w, "Link site").attributes("disabled")).toBeUndefined();
    await nameInput(w).setValue("---");
    expect(button(w, "Link site").attributes("disabled")).toBeDefined();
  });

  it("emits the slugified name, the folder, the PHP and HTTPS, then closes", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    await nameInput(w).setValue("My App");
    const radio = w.findAll('[role="radio"]').find((r) => r.text() === "8.4");
    await radio?.trigger("click");
    await w.get('[aria-label="Serve over HTTPS"]').trigger("click");
    expect(w.text()).toContain("On");
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")).toEqual([
      [{ name: "my-app", path: "/srv/app", php: "8.4", secure: true, applyProjectFile: false }],
    ]);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("preselects the default PHP in ascending order", () => {
    const w = mountDialog();
    const radios = w.findAll('[role="radio"]');
    expect(radios.map((r) => r.text())).toEqual(["8.3", "8.4"]);
    expect(radios[0]?.attributes("aria-checked")).toBe("true");
  });

  it("resets when closed", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    expect((pathInput(w).element as HTMLInputElement).value).toBe("");
    expect((nameInput(w).element as HTMLInputElement).value).toBe("");
  });

  it("shows the .yerd.toml banner for a chosen folder and emits an apply request", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    projectConfigDraft.mockResolvedValue(draft({ exists: true, name: "acme" }));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(projectConfigDraft).toHaveBeenCalledWith("/srv/app");
    expect(w.find('[data-testid="project-file-banner"]').exists()).toBe(true);
    expect(w.get('[data-testid="project-file-name"]').text()).toBe("acme.test");
    expect(w.find('[aria-label="Site name"]').exists()).toBe(false);
    expect(w.find('[aria-label="Serve over HTTPS"]').exists()).toBe(false);
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toMatchObject({
      name: "acme",
      path: "/srv/app",
      applyProjectFile: true,
    });
  });

  it("detects a .yerd.toml in a typed path once the field changes", async () => {
    projectConfigDraft.mockResolvedValue(draft({ exists: true, name: null }));
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "~/Code/My_Shop";
    await pathInput(w).trigger("input");
    expect(projectConfigDraft).not.toHaveBeenCalled();
    await pathInput(w).trigger("change");
    await flushPromises();
    expect(projectConfigDraft).toHaveBeenCalledWith("/Users/a/Code/My_Shop");
    expect(w.get('[data-testid="project-file-name"]').text()).toBe("my-shop.test");
  });

  it("disables Link while the folder's lookup is in flight", async () => {
    const pending = deferredDraft();
    pickDirectory.mockResolvedValue("/srv/app");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(button(w, "Link site").attributes("disabled")).toBeDefined();
    pending.resolve(draft({ exists: true, name: "acme" }));
    await flushPromises();
    expect(button(w, "Link site").attributes("disabled")).toBeUndefined();
    expect(w.get('[data-testid="project-file-name"]').text()).toBe("acme.test");
  });

  it("looks up a typed path that was never committed before emitting", async () => {
    const pending = deferredDraft();
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    expect(projectConfigDraft).not.toHaveBeenCalled();
    await button(w, "Link site").trigger("click");
    expect(projectConfigDraft).toHaveBeenCalledWith("/srv/app");
    expect(w.emitted("link")).toBeUndefined();
    pending.resolve(draft());
    await flushPromises();
    expect(w.emitted("link")).toEqual([
      [{ name: "typed", path: "/srv/app", php: "8.3", secure: false, applyProjectFile: false }],
    ]);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("stops at the banner when the awaited lookup finds an unseen .yerd.toml", async () => {
    const pending = deferredDraft();
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    await button(w, "Link site").trigger("click");
    pending.resolve(draft({ exists: true, name: "acme" }));
    await flushPromises();
    expect(w.emitted("link")).toBeUndefined();
    expect(w.emitted("update:open")).toBeUndefined();
    expect(w.find('[data-testid="project-file-banner"]').exists()).toBe(true);
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")).toEqual([
      [{ name: "acme", path: "/srv/app", php: "8.3", secure: false, applyProjectFile: true }],
    ]);
    expect(w.emitted("update:open")).toEqual([[false]]);
  });

  it("stops at the error when the awaited lookup finds an invalid .yerd.toml", async () => {
    projectConfigDraft.mockRejectedValue(new IpcError("unknown field `phpp`", "project_file_invalid"));
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    await button(w, "Link site").trigger("click");
    await flushPromises();
    expect(w.emitted("link")).toBeUndefined();
    expect(w.find('[data-testid="project-file-error"]').exists()).toBe(true);
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toMatchObject({ name: "typed", applyProjectFile: false });
  });

  it("emits once when Link is clicked again while the lookup runs", async () => {
    const pending = deferredDraft();
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    const link = button(w, "Link site").element as HTMLButtonElement;
    link.click();
    link.click();
    pending.resolve(draft());
    await flushPromises();
    expect(projectConfigDraft).toHaveBeenCalledTimes(1);
    expect(w.emitted("link")).toEqual([
      [{ name: "typed", path: "/srv/app", php: "8.3", secure: false, applyProjectFile: false }],
    ]);
  });

  it("drops a waiting submit when another folder is chosen meanwhile", async () => {
    let resolveFirst = (_value: ProjectConfig) => undefined as void;
    projectConfigDraft
      .mockReturnValueOnce(
        new Promise<ProjectConfig>((resolve) => {
          resolveFirst = resolve;
        }),
      )
      .mockResolvedValueOnce(draft({ file: "/srv/other/.yerd.toml" }));
    pickDirectory.mockResolvedValue("/srv/other");
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    await button(w, "Link site").trigger("click");
    await button(w, "Change").trigger("click");
    await flushPromises();
    resolveFirst(draft());
    await flushPromises();
    expect(w.emitted("link")).toBeUndefined();
    expect(w.emitted("update:open")).toBeUndefined();
  });

  it("drops a waiting submit when the dialog closes and reopens", async () => {
    let resolveFirst = (_value: ProjectConfig) => undefined as void;
    projectConfigDraft
      .mockReturnValueOnce(
        new Promise<ProjectConfig>((resolve) => {
          resolveFirst = resolve;
        }),
      )
      .mockResolvedValueOnce(draft());
    const w = mountDialog();
    (pathInput(w).element as HTMLInputElement).value = "/srv/app";
    await pathInput(w).trigger("input");
    await nameInput(w).setValue("typed");
    await button(w, "Link site").trigger("click");
    await w.setProps({ open: false });
    await w.setProps({ open: true });
    await pathInput(w).setValue("/srv/app");
    await pathInput(w).trigger("change");
    await nameInput(w).setValue("typed");
    await flushPromises();
    resolveFirst(draft());
    await flushPromises();
    expect(w.emitted("link")).toBeUndefined();
  });

  it("falls back to the site already at the folder for the read-only name", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    projectConfigDraft.mockResolvedValue(draft({ exists: true, name: null, site: "legacy" }));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="project-file-name"]').text()).toBe("legacy.test");
  });

  it("links the plain way when the apply switch is off", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    projectConfigDraft.mockResolvedValue(draft({ exists: true, name: "acme" }));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    await w.get('[aria-label="Apply .yerd.toml after linking"]').trigger("click");
    expect(w.find('[aria-label="Site name"]').exists()).toBe(true);
    expect(w.find('[aria-label="Serve over HTTPS"]').exists()).toBe(false);
    expect(w.get('[data-testid="project-file-defaults"]').text()).toContain("updates .yerd.toml");
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toEqual({
      name: "app",
      path: "/srv/app",
      php: null,
      secure: null,
      applyProjectFile: false,
    });
  });

  it("shows no banner for a folder without a .yerd.toml", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(w.find('[data-testid="project-file-banner"]').exists()).toBe(false);
    expect(w.find('[aria-label="Site name"]').exists()).toBe(true);
  });

  it("reports an invalid file inline and keeps the plain link available", async () => {
    pickDirectory.mockResolvedValue("/srv/app");
    projectConfigDraft.mockRejectedValue(new IpcError("unknown field `phpp`", "project_file_invalid"));
    const w = mountDialog();
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="project-file-error"]').text()).toContain("unknown field `phpp`");
    expect(button(w, "Link site").attributes("disabled")).toBeUndefined();
    await button(w, "Link site").trigger("click");
    expect(w.emitted("link")?.[0]?.[0]).toMatchObject({ applyProjectFile: false });
  });

  it("ignores other draft failures", async () => {
    projectConfigDraft.mockRejectedValue(new IpcError("no such folder", "invalid_path"));
    const w = mountDialog();
    await pathInput(w).setValue("/nope");
    await flushPromises();
    expect(w.find('[data-testid="project-file-error"]').exists()).toBe(false);
  });
});
