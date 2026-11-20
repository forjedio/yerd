import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

const pickDirectory = vi.fn();
vi.mock("@/ipc/client", () => ({
  pickDirectory: (...args: unknown[]) => pickDirectory(...args),
}));

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

beforeEach(() => {
  pickDirectory.mockReset();
});

describe("LinkFolderDialog", () => {
  it("prefills the name from the chosen folder and shows it relative to home", async () => {
    pickDirectory.mockResolvedValue("/Users/a/Code/Billing_API");
    const w = mountDialog();
    expect(w.text()).toContain("Pick the project folder to serve");
    await button(w, "Choose folder").trigger("click");
    await flushPromises();
    expect(w.get('[data-testid="link-path"]').text()).toBe("~/Code/Billing_API");
    expect((nameInput(w).element as HTMLInputElement).value).toBe("billing-api");
    expect(button(w, "Change").exists()).toBe(true);
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
    expect(w.emitted("link")).toEqual([[{ name: "my-app", path: "/srv/app", php: "8.4", secure: true }]]);
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
    expect(w.text()).toContain("Pick the project folder to serve");
    expect((nameInput(w).element as HTMLInputElement).value).toBe("");
  });
});
