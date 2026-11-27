import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import MailInbox from "./MailInbox.vue";
import type { MailStatus, MailSummary } from "@/ipc/types";
import type { AppOption } from "@/lib/mail";

const NOW = new Date(2026, 10, 19, 15, 0).getTime() / 1000;

function summary(id: string, overrides: Partial<MailSummary> = {}): MailSummary {
  return {
    id,
    from: "Codestash <hello@codestash.test>",
    to: ["sam@codestash.test"],
    subject: `Subject ${id}`,
    date_epoch: new Date(2026, 10, 19, 14, 32).getTime() / 1000,
    read: true,
    ...overrides,
  };
}

const MAILS = [summary("a", { read: false }), summary("b"), summary("c", { from: "Imagetome <ava@imagetome.test>" })];
const APPS: AppOption[] = [
  { value: "Codestash", label: "Codestash", count: 2 },
  { value: "Imagetome", label: "Imagetome", count: 1 },
];
const CAPTURE: MailStatus = { enabled: true, port: 2525, listening: true, count: 3 };

const Pass = { template: "<div><slot /></div>" };
const ItemStub = {
  emits: ["select"],
  template: '<div role="menuitem" @click="$emit(\'select\')"><slot /></div>',
};
const mounted: VueWrapper[] = [];

function mountInbox(props: Record<string, unknown> = {}) {
  const wrapper = mount(MailInbox, {
    attachTo: document.body,
    props: {
      layout: "list" as const,
      mails: MAILS,
      total: 3,
      unread: 1,
      apps: APPS,
      selected: "a",
      now: NOW,
      loading: false,
      error: null,
      clearing: false,
      app: "",
      "onUpdate:app": (v: string) => wrapper.setProps({ app: v }),
      ...props,
    },
    global: {
      stubs: {
        AlertDialogPortal: Pass,
        DropdownMenu: Pass,
        DropdownMenuTrigger: Pass,
        DropdownMenuContent: Pass,
        DropdownMenuItem: ItemStub,
      },
    },
  });
  mounted.push(wrapper);
  return wrapper;
}

type Inbox = ReturnType<typeof mountInbox>;

function row(w: Inbox, id: string) {
  return w.get(`[data-id="${id}"]`);
}

function buttonByText(w: Inbox, text: string) {
  const btn = w.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("MailInbox list", () => {
  it("shows the row fields, the unread dot and the selection", async () => {
    const w = mountInbox();
    expect(row(w, "a").text()).toContain("Codestash");
    expect(row(w, "a").text()).toContain("Subject a");
    expect(row(w, "a").text()).toContain("sam@codestash.test");
    expect(row(w, "a").text()).toContain("14:32");
    expect(row(w, "a").find('[aria-label="Unread"]').exists()).toBe(true);
    expect(row(w, "b").find('[aria-label="Unread"]').exists()).toBe(false);
    expect(row(w, "a").attributes("aria-current")).toBe("true");
    expect(w.get('[data-testid="unread-chip"]').text()).toBe("1 unread");
    await row(w, "b").trigger("click");
    expect(w.emitted("select")).toEqual([["b"]]);
  });

  it("hides the unread chip at zero", () => {
    const w = mountInbox({ unread: 0 });
    expect(w.find('[data-testid="unread-chip"]').exists()).toBe(false);
  });
});

describe("MailInbox table", () => {
  it("shows the columns and the app chip with no persistent selection", () => {
    const w = mountInbox({ layout: "table" });
    expect(w.text()).toContain("Subject");
    expect(w.text()).toContain("Received");
    expect(row(w, "c").get('[data-testid="app-chip"]').text()).toBe("Imagetome");
    expect(row(w, "a").attributes("aria-current")).toBeUndefined();
    expect(row(w, "a").classes()).not.toContain("bg-brand/[0.14]");
  });
});

describe("MailInbox header", () => {
  it("filters by app from the menu", async () => {
    const w = mountInbox();
    const items = w.findAll('[role="menuitem"]');
    expect(items.map((i) => i.text())).toEqual(["All apps3", "Codestash2", "Imagetome1"]);
    await items[2]?.trigger("click");
    expect(w.props("app")).toBe("Imagetome");
  });

  it("confirms Clear all", async () => {
    const w = mountInbox();
    await w.get('[aria-label="Clear all messages"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Clear all 3 messages?");
    (Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Cancel") as HTMLButtonElement).click();
    await flushPromises();
    expect(w.emitted("clearAll")).toBeUndefined();
    await w.get('[aria-label="Clear all messages"]').trigger("click");
    await flushPromises();
    (Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find((b) => b.textContent?.trim() === "Clear all") as HTMLButtonElement).click();
    await flushPromises();
    expect(w.emitted("clearAll")).toHaveLength(1);
  });

  it("words the Clear all title for a single message", async () => {
    const w = mountInbox({ mails: [summary("a")], total: 1, unread: 0 });
    await w.get('[aria-label="Clear all messages"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Clear 1 message?");
    expect(document.body.textContent).not.toContain("Clear all 1");
  });

  it("disables the filter and Clear all when empty", () => {
    const w = mountInbox({ mails: [], total: 0, unread: 0, apps: [] });
    expect(w.get('[aria-label="Clear all messages"]').attributes("disabled")).toBeDefined();
    expect(w.get('[data-testid="app-filter"]').attributes("disabled")).toBeDefined();
  });

  it("keeps the filter usable when an active app filter outlives the mailbox", () => {
    const w = mountInbox({ mails: [], total: 0, unread: 0, apps: [], app: "Laravel" });
    expect(w.get('[data-testid="app-filter"]').attributes("disabled")).toBeUndefined();
  });
});

describe("MailInbox states", () => {
  const empty = { mails: [], total: 0, unread: 0, apps: [] };

  it("offers Turn on when capture is off", async () => {
    const w = mountInbox({ ...empty, capture: { ...CAPTURE, enabled: false, listening: false } });
    expect(w.text()).toContain("Mail capture is off");
    await buttonByText(w, "Turn on").trigger("click");
    expect(w.emitted("turnOn")).toHaveLength(1);
  });

  it("offers the .env when capture is on", async () => {
    const w = mountInbox({ ...empty, capture: CAPTURE });
    expect(w.text()).toContain("127.0.0.1:2525");
    await buttonByText(w, "Copy Laravel .env").trigger("click");
    expect(w.emitted("copyEnv")).toHaveLength(1);
  });

  it("uses generic copy without a capture status", () => {
    const w = mountInbox(empty);
    expect(w.text()).toContain("No mail yet");
    expect(w.text()).toContain("Yerd's SMTP server");
    expect(w.findAll("button").some((b) => b.text().includes("Copy Laravel .env"))).toBe(false);
  });

  it("offers Show all apps for an empty filter", async () => {
    const w = mountInbox({ mails: [], app: "Laravel" });
    expect(w.text()).toContain("No mail from Laravel");
    await buttonByText(w, "Show all apps").trigger("click");
    expect(w.props("app")).toBe("");
  });

  it("offers Retry on a load error", async () => {
    const w = mountInbox({ ...empty, error: "daemon unreachable" });
    expect(w.text()).toContain("daemon unreachable");
    await buttonByText(w, "Retry").trigger("click");
    expect(w.emitted("retry")).toHaveLength(1);
  });
});
