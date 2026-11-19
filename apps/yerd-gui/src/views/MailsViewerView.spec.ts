import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));

import MailsViewerView from "./MailsViewerView.vue";
import type { MailDetail, MailSummary } from "@/ipc/types";

function summary(id: string, from = "Codestash <hello@codestash.test>"): MailSummary {
  return { id, from, to: ["sam@codestash.test"], subject: `Subject ${id}`, date_epoch: 1_700_000_000, read: false };
}

function detailOf(id: string): MailDetail {
  return { id, from: "Codestash <hello@codestash.test>", to: [], subject: `Subject ${id}`, date_epoch: 0, headers: [], html_body: null, text_body: "hi" };
}

let mails: MailSummary[] = [];

const Pass = { template: "<div><slot /></div>" };
const ItemStub = {
  emits: ["select"],
  template: '<div role="menuitem" @click="$emit(\'select\')"><slot /></div>',
};
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(MailsViewerView, {
    attachTo: document.body,
    global: {
      stubs: {
        TitleBar: true,
        AlertDialogPortal: Pass,
        DropdownMenu: Pass,
        DropdownMenuTrigger: Pass,
        DropdownMenuContent: Pass,
        DropdownMenuItem: ItemStub,
      },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

beforeEach(() => {
  invokeMock.mockReset();
  mails = [summary("a"), summary("b", "Imagetome <ava@imagetome.test>")];
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "list_mails":
        return Promise.resolve({ type: "mails", mails });
      case "get_mail":
        return Promise.resolve({ type: "mail", mail: detailOf(String(args?.id)) });
      case "mark_mails_read":
      case "delete_mails":
        return Promise.resolve({ type: "ok" });
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("MailsViewerView", () => {
  it("auto-selects the first message unread, marks a clicked one read and lists apps", async () => {
    const w = await mountView();
    expect(w.get('[data-id="a"]').attributes("aria-current")).toBe("true");
    expect(calls("mark_mails_read")).toHaveLength(0);
    await w.get('[data-id="b"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("mark_mails_read", { ids: ["b"] });
    expect(w.findAll('[role="menuitem"]').map((i) => i.text())).toEqual(["All apps2", "Codestash1", "Imagetome1"]);
  });

  it("deletes the open message after confirming", async () => {
    const w = await mountView();
    await w.findAll("button").find((b) => b.text() === "Delete")?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find(
      (b) => b.textContent?.trim() === "Delete",
    ) as HTMLButtonElement;
    confirm.click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("delete_mails", { ids: ["a"] });
  });

  it("shows the generic empty state without a Copy button", async () => {
    mails = [];
    const w = await mountView();
    expect(w.text()).toContain("No mail yet");
    expect(w.text()).not.toContain("Copy Laravel .env");
  });
});
