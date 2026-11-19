import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));

const push = vi.fn();
vi.mock("vue-router", () => ({
  useRouter: () => ({ push }),
}));

import MailView from "./MailView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import type { MailDetail, MailStatus, MailSummary, StatusReport } from "@/ipc/types";

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
  return { id, from: "Codestash <hello@codestash.test>", to: ["sam@codestash.test"], subject: `Subject ${id}`, date_epoch: 0, headers: [], html_body: null, text_body: "hi" };
}

const CAPTURE: MailStatus = { enabled: true, port: 2525, listening: true, count: 2 };

let mails: MailSummary[] = [];

function seed(mail: MailStatus | null, bootId?: number): void {
  useDaemon().report.value = { tld: "test", mail, boot_id: bootId } as unknown as StatusReport;
}

function stubIpc(): void {
  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "list_mails":
        return Promise.resolve({ type: "mails", mails });
      case "get_mail":
        return Promise.resolve({ type: "mail", mail: detailOf(String(args?.id)) });
      case "set_mail_enabled": {
        const current = useDaemon().report.value;
        seed({ ...(current?.mail ?? CAPTURE), enabled: Boolean(args?.enabled) }, current?.boot_id ?? undefined);
        return Promise.resolve({ type: "ok" });
      }
      case "status":
        return Promise.resolve({ type: "status", report: useDaemon().report.value });
      case "mark_mails_read":
      case "delete_mails":
      case "clear_mails":
        return Promise.resolve({ type: "ok" });
      case "show_mails_window":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

function setWide(wide: boolean): void {
  globalThis.matchMedia = vi.fn(() => ({
    matches: wide,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as unknown as typeof globalThis.matchMedia;
}

const Pass = { template: "<div><slot /></div>" };
const ItemStub = {
  emits: ["select"],
  template: '<div role="menuitem" @click="$emit(\'select\')"><slot /></div>',
};
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(MailView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: true,
        RouterLink: true,
        DialogPortal: Pass,
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

type View = Awaited<ReturnType<typeof mountView>>;

function pill(w: View): string {
  return w.get('[data-testid="capture-pill"]').text();
}

function bodyButton(text: string, scope = ""): HTMLButtonElement {
  const btn = Array.from(document.body.querySelectorAll(`${scope} button`)).find((b) => b.textContent?.trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn as HTMLButtonElement;
}

function reader(w: View) {
  return w.find('[data-testid="mail-reader"]');
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

function escape(): void {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
}

beforeEach(() => {
  invokeMock.mockReset();
  push.mockReset();
  mails = [summary("a"), summary("b")];
  seed(CAPTURE);
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  setWide(true);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("MailView capture pill", () => {
  it.each([
    [CAPTURE, "Capturing on 2525"],
    [{ ...CAPTURE, listening: false }, "Port 2525 in use"],
    [{ ...CAPTURE, enabled: false, listening: false }, "Capture off"],
  ])("reads %j", async (status, label) => {
    seed(status);
    const w = await mountView();
    expect(pill(w)).toBe(label);
  });

  it("asks for a restart after a toggle in the same boot, across a remount", async () => {
    seed(CAPTURE, 7);
    const first = await mountView();
    await first.findAll("button").find((b) => b.text() === "Setup")?.trigger("click");
    await flushPromises();
    (document.body.querySelector('[aria-label="Capture outgoing mail"]') as HTMLButtonElement).click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_mail_enabled", { enabled: false });
    expect(toastTitles()).toContain("Mail capture turned off");
    expect(pill(first)).toBe("Restart Yerd to apply");
    first.unmount();
    mounted.length = 0;
    document.body.innerHTML = "";
    const second = await mountView();
    expect(pill(second)).toBe("Restart Yerd to apply");
    seed({ ...CAPTURE, enabled: false, listening: true }, 8);
    await flushPromises();
    expect(pill(second)).toBe("Capture off");
  });
});

describe("MailView toolbar and setup", () => {
  it("copies the .env and sends Change port to Settings", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const w = await mountView();
    await w.findAll("button").find((b) => b.text() === "Setup")?.trigger("click");
    await flushPromises();
    const address = document.body.querySelector<HTMLInputElement>("#mail-from-address");
    if (!address) throw new Error("no From address input");
    address.value = "shop@x.test";
    address.dispatchEvent(new Event("input"));
    await flushPromises();
    (document.body.querySelector('[aria-label="Copy .env configuration"]') as HTMLButtonElement).click();
    await flushPromises();
    expect(writeText.mock.calls[0]?.[0]).toContain("MAIL_PORT=2525");
    expect(writeText.mock.calls[0]?.[0]).toContain('MAIL_FROM_ADDRESS="shop@x.test"');
    expect(toastTitles()).toContain("Copied to clipboard");
    bodyButton("Change port").click();
    await flushPromises();
    expect(push).toHaveBeenCalledWith("/general");
    expect(document.body.textContent).not.toContain("Capture outgoing mail");
  });

  it("opens the Mails window", async () => {
    const w = await mountView();
    await w.get('[aria-label="Open in a separate window"]').trigger("click");
    await flushPromises();
    expect(calls("show_mails_window")).toHaveLength(1);
  });
});

describe("MailView wide", () => {
  it("auto-selects without marking read, and a click marks read", async () => {
    const w = await mountView();
    expect(calls("get_mail")[0]?.[1]).toEqual({ id: "a" });
    expect(calls("mark_mails_read")).toHaveLength(0);
    expect(reader(w).text()).toContain("Subject a");
    await w.get('[data-id="b"]').trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("mark_mails_read", { ids: ["b"] });
    expect(w.get('[data-id="b"]').find('[aria-label="Unread"]').exists()).toBe(false);
    expect(w.get('[data-testid="unread-chip"]').text()).toBe("1 unread");
  });
});

describe("MailView compact drawer", () => {
  beforeEach(() => setWide(false));

  it("opens on a row click and closes from its button", async () => {
    const w = await mountView();
    expect(reader(w).exists()).toBe(false);
    expect(calls("get_mail")).toHaveLength(0);
    await w.get('[data-id="b"]').trigger("click");
    await flushPromises();
    expect(reader(w).text()).toContain("Subject b");
    await w.get('[aria-label="Close message"]').trigger("click");
    await flushPromises();
    expect(reader(w).exists()).toBe(false);
  });

  it("deletes from the drawer and closes it", async () => {
    const w = await mountView();
    await w.get('[data-id="a"]').trigger("click");
    await flushPromises();
    bodyButton("Delete", '[data-testid="mail-reader"]').click();
    await flushPromises();
    bodyButton("Delete", '[role="alertdialog"]').click();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("delete_mails", { ids: ["a"] });
    expect(toastTitles()).toContain("Message deleted");
    expect(reader(w).exists()).toBe(false);
  });

  it("lets Escape close only the confirm above the drawer", async () => {
    const w = await mountView();
    await w.get('[data-id="a"]').trigger("click");
    await flushPromises();
    bodyButton("Delete", '[data-testid="mail-reader"]').click();
    await flushPromises();
    escape();
    await flushPromises();
    expect(document.body.querySelector('[role="alertdialog"]')).toBeNull();
    expect(reader(w).exists()).toBe(true);
  });
});

describe("MailView empty states", () => {
  beforeEach(() => {
    mails = [];
  });

  it("turns capture on from the empty state", async () => {
    seed({ ...CAPTURE, enabled: false, listening: false, count: 0 });
    const w = await mountView();
    await w.findAll("button").find((b) => b.text() === "Turn on")?.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_mail_enabled", { enabled: true });
  });

  it("copies the .env from the empty state", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const w = await mountView();
    await w.findAll("button").find((b) => b.text() === "Copy Laravel .env")?.trigger("click");
    await flushPromises();
    expect(writeText.mock.calls[0]?.[0]).toContain("MAIL_MAILER=smtp");
  });
});
