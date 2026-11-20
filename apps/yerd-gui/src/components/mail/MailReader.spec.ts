import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const openUrl = vi.fn();
const openPath = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => openUrl(...args),
  openPath: (...args: unknown[]) => openPath(...args),
}));

import MailReader from "./MailReader.vue";
import { useToast } from "@/composables/useToast";
import type { MailDetail } from "@/ipc/types";

const NOW = new Date(2026, 10, 19, 15, 0).getTime() / 1000;

function detail(overrides: Partial<MailDetail> = {}): MailDetail {
  return {
    id: "m1",
    from: "Codestash <hello@codestash.test>",
    to: ["sam@codestash.test"],
    subject: "Reset Password Notification",
    date_epoch: new Date(2026, 10, 19, 14, 32).getTime() / 1000,
    headers: [
      { name: "Message-ID", value: "<a91f@codestash.test>" },
      { name: "X-Mailer", value: "Symfony Mailer" },
    ],
    html_body: '<p id="sentinel">Hello!</p><img src="https://codestash.test/img/logo.png">',
    text_body: "Hello! Visit https://codestash.test/reset",
    ...overrides,
  };
}

const TEXT_ONLY = detail({ id: "m2", html_body: null, subject: "Plain" });

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountReader(props: Record<string, unknown> = {}) {
  const wrapper = mount(MailReader, {
    attachTo: document.body,
    props: { detail: detail(), loading: false, now: NOW, deleting: false, ...props },
    global: { stubs: { AlertDialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type Reader = Awaited<ReturnType<typeof mountReader>>;

function shadowHtml(w: Reader): string {
  return (w.get('[data-testid="reader-html"]').element as HTMLElement).shadowRoot?.innerHTML ?? "";
}

function tab(w: Reader, name: string) {
  const t = w.findAll('[role="tab"]').find((b) => b.text() === name);
  if (!t) throw new Error(`no tab ${name}`);
  return t;
}

async function openTab(w: Reader, name: string): Promise<void> {
  await tab(w, name).trigger("mousedown", { button: 0, ctrlKey: false });
  await flushPromises();
}

function button(text: string): HTMLButtonElement {
  const btn = Array.from(document.body.querySelectorAll("button")).find((b) => b.textContent?.trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

beforeEach(() => {
  invokeMock.mockReset();
  openUrl.mockReset().mockResolvedValue(undefined);
  openPath.mockReset().mockResolvedValue(undefined);
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("MailReader", () => {
  it("shows the header and renders HTML into the shadow root", async () => {
    const w = await mountReader();
    expect(w.get('[data-testid="reader-subject"]').text()).toBe("Reset Password Notification");
    expect(w.get('[data-testid="reader-meta"]').text()).toContain("<hello@codestash.test>");
    expect(w.get('[data-testid="reader-meta"]').text()).toContain("Today 14:32");
    expect(tab(w, "Preview").attributes("data-state")).toBe("active");
    expect(shadowHtml(w)).toContain('id="sentinel"');
  });

  it("opens a text-only mail on Text with Preview disabled", async () => {
    const w = await mountReader({ detail: TEXT_ONLY });
    expect(tab(w, "Text").attributes("data-state")).toBe("active");
    expect(tab(w, "Preview").attributes("disabled")).toBeDefined();
  });

  it("lists headers and the remote content", async () => {
    const w = await mountReader();
    await openTab(w, "Headers");
    const text = w.get('[data-testid="reader-headers"]').text();
    expect(text).toContain("Message-ID");
    expect(text).toContain("Symfony Mailer");
    expect(text).toContain("https://codestash.test/img/logo.png");
  });

  it("blocks remote images until Load and resets on a new message", async () => {
    const w = await mountReader();
    expect(w.get('[data-testid="remote-bar"]').text()).toContain("1 remote image blocked");
    expect(shadowHtml(w)).not.toContain("https://codestash.test/img/logo.png");
    await w.get('[data-testid="remote-bar"] button').trigger("click");
    await flushPromises();
    expect(shadowHtml(w)).toContain("https://codestash.test/img/logo.png");
    expect(w.get('[data-testid="remote-bar"]').text()).toContain("Remote content loaded");
    await openTab(w, "Headers");
    await w.setProps({ detail: detail({ id: "m3" }) });
    await flushPromises();
    expect(tab(w, "Preview").attributes("data-state")).toBe("active");
    expect(w.get('[data-testid="remote-bar"]').text()).toContain("1 remote image blocked");
    expect(shadowHtml(w)).not.toContain("https://codestash.test/img/logo.png");
  });

  it("opens a link clicked in the text body", async () => {
    const w = await mountReader({ detail: TEXT_ONLY });
    await w.get('[data-testid="reader-text"] a').trigger("click");
    await flushPromises();
    expect(openUrl).toHaveBeenCalledWith("https://codestash.test/reset");
  });

  it("saves and opens an attachment, and toasts a failure", async () => {
    invokeMock.mockResolvedValueOnce("/tmp/INV-1042.pdf").mockRejectedValueOnce(new Error("disk full"));
    const att = { filename: "INV-1042.pdf", content_type: "application/pdf", size: 49152, data: "AAA=" };
    const w = await mountReader({ detail: detail({ attachments: [att] }) });
    expect(w.get('[data-testid="reader-attachments"]').text()).toContain("1 attachment");
    expect(w.get('[data-testid="reader-attachments"]').text()).toContain("PDF · 48");
    const tile = w.get('[data-testid="reader-attachments"] button');
    await tile.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("save_mail_attachment", { filename: "INV-1042.pdf", data: "AAA=" });
    expect(openPath).toHaveBeenCalledWith("/tmp/INV-1042.pdf");
    await tile.trigger("click");
    await flushPromises();
    expect(useToast().toasts.value.map((t) => t.title)).toContain("Couldn't open attachment");
  });

  it("confirms Delete for the message it was opened on", async () => {
    const w = await mountReader();
    await w.findAll("button").find((b) => b.text() === "Delete")?.trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("“Reset Password Notification” will be deleted.");
    button("Cancel").click();
    await flushPromises();
    expect(w.emitted("delete")).toBeUndefined();
    await w.findAll("button").find((b) => b.text() === "Delete")?.trigger("click");
    await flushPromises();
    await w.setProps({ detail: detail({ id: "other" }) });
    const confirm = Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find(
      (b) => b.textContent?.trim() === "Delete",
    ) as HTMLButtonElement;
    confirm.click();
    await flushPromises();
    expect(w.emitted("delete")).toEqual([["m1"]]);
  });

  it("offers Delete for a message whose body could not be read", async () => {
    const w = await mountReader({ detail: null, failedId: "broken" });
    expect(w.get('[data-testid="reader-failed"]').text()).toContain("couldn't be opened");
    await w.get('[data-testid="reader-failed"] button').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("“(unreadable message)” will be deleted.");
    const confirm = Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find(
      (b) => b.textContent?.trim() === "Delete",
    ) as HTMLButtonElement;
    confirm.click();
    await flushPromises();
    expect(w.emitted("delete")).toEqual([["broken"]]);
  });

  it.each([
    ["a bare # link", '<a id="link" href="#">Unsubscribe</a>'],
    ["a #fragment link", '<p id="top">Top</p><a id="link" href="#top">Back to top</a>'],
    ["a missing #fragment", '<a id="link" href="#nowhere">Nowhere</a>'],
    ["a relative link", '<a id="link" href="/account">Account</a>'],
  ])("cancels %s so the app route never changes", async (_, html) => {
    window.location.hash = "#/mail";
    const w = await mountReader({ detail: detail({ html_body: html }) });
    const root = (w.get('[data-testid="reader-html"]').element as HTMLElement).shadowRoot;
    const link = root?.getElementById("link");
    if (!link) throw new Error("no link");
    const click = new MouseEvent("click", { bubbles: true, cancelable: true, composed: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(window.location.hash).toBe("#/mail");
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("scrolls the reader frame to a #fragment target", async () => {
    const w = await mountReader({ detail: detail({ html_body: '<a id="link" href="#end">Jump</a><p id="end">End</p>' }) });
    const frame = w.get('[data-testid="reader-frame"]').element as HTMLElement;
    const root = (w.get('[data-testid="reader-html"]').element as HTMLElement).shadowRoot;
    const target = root?.getElementById("end");
    if (!target) throw new Error("no target");
    vi.spyOn(frame, "getBoundingClientRect").mockReturnValue({ top: 100 } as DOMRect);
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue({ top: 420 } as DOMRect);
    root?.getElementById("link")?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true }));
    expect(frame.scrollTop).toBe(320);
  });

  it("contains the message so its CSS cannot cover the app", async () => {
    const w = await mountReader({
      detail: detail({
        html_body:
          '<html><head><style>:host { position: fixed !important; inset: 0 !important; }</style></head><body><div style="position:fixed;inset:0">x</div></body></html>',
      }),
    });
    const frame = w.get('[data-testid="reader-frame"]').element as HTMLElement;
    expect(frame.style.getPropertyValue("contain")).toBe("layout paint");
    expect(frame.style.isolation).toBe("isolate");
    expect(frame.style.transform).toBe("translateZ(0)");
    expect(frame.contains(w.get('[data-testid="reader-html"]').element)).toBe(true);
    expect(shadowHtml(w)).not.toMatch(/:host\s*\{\s*position/);
    expect(shadowHtml(w)).toContain(":not(*) { position: fixed !important;");
  });

  it("shows a close button when closable and disables Delete while deleting", async () => {
    const w = await mountReader({ closable: true, deleting: true });
    await w.get('[aria-label="Close message"]').trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
    expect(w.findAll("button").find((b) => b.text() === "Delete")?.attributes("disabled")).toBeDefined();
  });
});
