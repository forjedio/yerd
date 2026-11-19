import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, ref, type Ref } from "vue";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { type Mailbox, useMailbox } from "./useMailbox";
import { useToast } from "@/composables/useToast";
import type { MailDetail, MailSummary } from "@/ipc/types";

function summary(id: string, overrides: Partial<MailSummary> = {}): MailSummary {
  return {
    id,
    from: "Codestash <hello@codestash.test>",
    to: ["sam@codestash.test"],
    subject: `Subject ${id}`,
    date_epoch: 1_700_000_000,
    read: false,
    ...overrides,
  };
}

function detailOf(id: string): MailDetail {
  return { id, from: "Codestash <hello@codestash.test>", to: [], subject: `Subject ${id}`, date_epoch: 0, headers: [], html_body: null, text_body: "hi" };
}

let mails: MailSummary[] = [];
let getMailImpl: (id: string) => Promise<unknown> = (id) => Promise.resolve({ type: "mail", mail: detailOf(id) });

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "list_mails":
        return Promise.resolve({ type: "mails", mails });
      case "get_mail":
        return getMailImpl(String(args?.id));
      case "mark_mails_read":
      case "delete_mails":
      case "clear_mails":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: VueWrapper[] = [];

async function setup(autoSelect: Ref<boolean> = ref(true)): Promise<Mailbox> {
  let box: Mailbox | null = null;
  const Host = defineComponent({
    setup() {
      box = useMailbox({ autoSelect: () => autoSelect.value });
      return () => h("div");
    },
  });
  mounted.push(mount(Host));
  await flushPromises();
  if (!box) throw new Error("mailbox not created");
  return box;
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

async function poll(): Promise<void> {
  await vi.advanceTimersByTimeAsync(4000);
  await flushPromises();
}

beforeEach(() => {
  invokeMock.mockReset();
  mails = [summary("a"), summary("b")];
  getMailImpl = (id) => Promise.resolve({ type: "mail", mail: detailOf(id) });
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  vi.useRealTimers();
});

describe("useMailbox selection", () => {
  it("auto-selects the first message without marking it read", async () => {
    const box = await setup();
    expect(box.selectedId.value).toBe("a");
    expect(box.detail.value?.id).toBe("a");
    expect(calls("mark_mails_read")).toHaveLength(0);
    expect(box.unread.value).toBe(2);
  });

  it("selects nothing on its own when auto-select is off, until it turns on", async () => {
    const auto = ref(false);
    const box = await setup(auto);
    expect(box.selectedId.value).toBeNull();
    expect(calls("get_mail")).toHaveLength(0);
    auto.value = true;
    await flushPromises();
    expect(box.selectedId.value).toBe("a");
  });

  it("marks a user selection read locally", async () => {
    const box = await setup();
    await box.select("b", true);
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("mark_mails_read", { ids: ["b"] });
    expect(box.list.value.find((m) => m.id === "b")?.read).toBe(true);
    expect(box.unread.value).toBe(1);
  });

  it("drops a slower earlier get_mail reply", async () => {
    const box = await setup();
    let release: (v: unknown) => void = () => undefined;
    getMailImpl = (id) =>
      id === "a"
        ? new Promise((resolve) => {
            release = resolve;
          })
        : Promise.resolve({ type: "mail", mail: detailOf(id) });
    const slow = box.select("a", true);
    await box.select("b", true);
    release({ type: "mail", mail: detailOf("a") });
    await slow;
    await flushPromises();
    expect(box.selectedId.value).toBe("b");
    expect(box.detail.value?.id).toBe("b");
  });

  it("drops a user's pick that fails to load but keeps a failed automatic one", async () => {
    getMailImpl = () => Promise.reject(new Error("gone"));
    const box = await setup();
    expect(box.selectedId.value).toBe("a");
    expect(box.detail.value).toBeNull();
    await box.select("b", true);
    expect(box.selectedId.value).toBeNull();
    expect(box.detail.value).toBeNull();
    expect(box.loadingDetail.value).toBe(false);
    expect(toastTitles()).toContain("Couldn't open the email");
  });

  it("moves to the next message when the selected one disappears", async () => {
    vi.useFakeTimers();
    const box = await setup();
    mails = [summary("b")];
    await poll();
    expect(box.selectedId.value).toBe("b");
    expect(box.detail.value?.id).toBe("b");
  });
});

describe("useMailbox filter", () => {
  it("keeps an app filter whose mail is gone and lists it with 0", async () => {
    vi.useFakeTimers();
    mails = [summary("a"), summary("b", { from: "Imagetome <ava@imagetome.test>" })];
    const box = await setup();
    box.app.value = "Imagetome";
    await flushPromises();
    expect(box.filtered.value.map((m) => m.id)).toEqual(["b"]);
    expect(box.selectedId.value).toBe("b");
    mails = [summary("a")];
    await poll();
    expect(box.app.value).toBe("Imagetome");
    expect(box.filtered.value).toEqual([]);
    expect(box.selectedId.value).toBeNull();
    expect(box.apps.value).toContainEqual({ value: "Imagetome", label: "Imagetome", count: 0 });
  });
});

describe("useMailbox delete and clear", () => {
  it("deletes one message and toasts", async () => {
    const box = await setup();
    expect(await box.deleteMail("a")).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("delete_mails", { ids: ["a"] });
    expect(toastTitles()).toContain("Message deleted");
  });

  it("toasts a failed delete and returns false", async () => {
    const box = await setup();
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "delete_mails" ? Promise.reject(new Error("nope")) : Promise.resolve({ type: "mails", mails }),
    );
    expect(await box.deleteMail("a")).toBe(false);
    expect(toastTitles()).toContain("Couldn't delete the message");
  });

  it("clears the mailbox and the app filter", async () => {
    const box = await setup();
    box.app.value = "Codestash";
    expect(await box.clearAll()).toBe(true);
    expect(calls("clear_mails")).toHaveLength(1);
    expect(box.app.value).toBe("");
    expect(toastTitles()).toContain("Mailbox cleared");
  });
});
