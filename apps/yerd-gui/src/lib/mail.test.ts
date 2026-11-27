import { describe, expect, it } from "vitest";

import {
  appOptions,
  attachmentsLabel,
  captureStatus,
  filterByApp,
  laravelEnv,
  listTime,
  mailApp,
  mimeLabel,
  readerTime,
  remoteKindLabel,
  remoteSummary,
  senderParts,
  unreadCount,
} from "./mail";
import type { MailStatus, MailSummary } from "@/ipc/types";
import type { RemoteContentRef } from "@/lib/mailLinks";

function mail(overrides: Partial<MailSummary> = {}): MailSummary {
  return { id: "1", from: "Codestash <hello@codestash.test>", to: ["sam@codestash.test"], subject: "Hi", date_epoch: 1, ...overrides };
}

function epoch(d: Date): number {
  return d.getTime() / 1000;
}

const NOW = epoch(new Date(2026, 10, 19, 15, 0));

describe("senderParts", () => {
  it.each([
    ["Codestash <hello@codestash.test>", { name: "Codestash", address: "hello@codestash.test" }],
    ['"Code Stash" <hello@codestash.test>', { name: "Code Stash", address: "hello@codestash.test" }],
    ["hello@codestash.test", { name: "", address: "hello@codestash.test" }],
    ["<hello@codestash.test>", { name: "", address: "hello@codestash.test" }],
    ["", { name: "", address: "" }],
    ["Broken <hello@codestash.test", { name: "", address: "Broken <hello@codestash.test" }],
  ])("splits %j", (from, expected) => {
    expect(senderParts(from)).toEqual(expected);
  });
});

describe("mailApp", () => {
  it.each([
    [mail(), "Codestash"],
    [mail({ from: "hello@blog.test", site: "blog" }), "blog"],
    [mail({ from: "hello@blog.test" }), "hello@blog.test"],
    [mail({ from: "" }), "(unknown sender)"],
  ])("labels %j", (m, expected) => {
    expect(mailApp(m)).toBe(expected);
  });
});

describe("app filter", () => {
  const list = [
    mail({ id: "1", from: "Imagetome <a@i.test>" }),
    mail({ id: "2" }),
    mail({ id: "3", from: "Imagetome <b@i.test>", read: false }),
    mail({ id: "4", read: true }),
  ];

  it("counts and sorts the apps", () => {
    expect(appOptions(list, "")).toEqual([
      { value: "Codestash", label: "Codestash", count: 2 },
      { value: "Imagetome", label: "Imagetome", count: 2 },
    ]);
  });

  it("keeps a missing active filter with a zero count", () => {
    expect(appOptions(list, "Laravel")[2]).toEqual({ value: "Laravel", label: "Laravel", count: 0 });
    expect(appOptions([], "")).toEqual([]);
  });

  it("filters by app", () => {
    expect(filterByApp(list, "Imagetome").map((m) => m.id)).toEqual(["1", "3"]);
    expect(filterByApp(list, "")).toHaveLength(4);
  });

  it("counts only explicit unread", () => {
    expect(unreadCount(list)).toBe(1);
  });
});

describe("times", () => {
  it.each([
    [new Date(2026, 10, 19, 14, 32), "14:32", "Today 14:32"],
    [new Date(2026, 10, 18, 23, 59), "Yesterday", "Yesterday 23:59"],
    [new Date(2026, 10, 16, 9, 5), "Mon", "Mon 09:05"],
    [new Date(2026, 10, 12, 11, 48), "12 Nov", "12 Nov 11:48"],
    [new Date(2026, 1, 4, 8, 0), "4 Feb", "4 Feb 08:00"],
    [new Date(2025, 10, 4, 11, 48), "4 Nov 2025", "4 Nov 2025, 11:48"],
  ])("formats %s", (date, list, reader) => {
    expect(listTime(epoch(date), NOW)).toBe(list);
    expect(readerTime(epoch(date), NOW)).toBe(reader);
  });

  it("shows a dash without a date", () => {
    expect(listTime(0, NOW)).toBe("-");
    expect(readerTime(0, NOW)).toBe("-");
  });
});

describe("captureStatus", () => {
  const status = (o: Partial<MailStatus>): MailStatus => ({ enabled: true, port: 2525, listening: true, count: 0, ...o });

  it.each([
    [null, false, "off", "Capture unknown"],
    [status({}), false, "ok", "Capturing on 2525"],
    [status({ listening: false }), false, "warn", "Port 2525 in use"],
    [status({ enabled: false, listening: false }), false, "off", "Capture off"],
    [status({ listening: false }), true, "pending", "Restart Yerd to apply"],
    [status({ enabled: false }), true, "pending", "Restart Yerd to apply"],
    [status({}), true, "ok", "Capturing on 2525"],
    [status({ enabled: false, listening: false }), true, "off", "Capture off"],
  ])("reads %j (pending %s)", (m, pending, tone, label) => {
    expect(captureStatus(m, pending)).toMatchObject({ tone, label });
  });

  it("explains a pending change", () => {
    expect(captureStatus(status({ enabled: false }), true).title).toBe(
      "Capture was turned off; it takes effect when Yerd restarts.",
    );
  });
});

describe("laravelEnv", () => {
  it("keeps the snippet byte for byte", () => {
    expect(laravelEnv({ port: 2525, fromName: "", fromAddress: "  " })).toBe(
      'MAIL_MAILER=smtp\nMAIL_HOST=127.0.0.1\nMAIL_PORT=2525\nMAIL_USERNAME=null\nMAIL_PASSWORD=null\nMAIL_ENCRYPTION=null\nMAIL_FROM_ADDRESS="hello@example.com"\nMAIL_FROM_NAME="${APP_NAME}"',
    );
  });

  it("uses the given name, address and port", () => {
    const env = laravelEnv({ port: 2526, fromName: " Shop ", fromAddress: " shop@x.test " });
    expect(env).toContain("MAIL_PORT=2526");
    expect(env).toContain('MAIL_FROM_ADDRESS="shop@x.test"');
    expect(env).toContain('MAIL_FROM_NAME="Shop"');
  });

  it("escapes quotes and backslashes in the quoted values", () => {
    const env = laravelEnv({ port: 2525, fromName: 'The "Best" \\ Shop', fromAddress: 'a"b@x.test' });
    expect(env).toContain('MAIL_FROM_NAME="The \\"Best\\" \\\\ Shop"');
    expect(env).toContain('MAIL_FROM_ADDRESS="a\\"b@x.test"');
  });
});

describe("remote content and attachments", () => {
  const ref = (kind: RemoteContentRef["kind"], url = "https://x.test/a"): RemoteContentRef => ({ kind, url });

  it.each([
    [[], false, null],
    [[ref("image")], false, "1 remote image blocked"],
    [[ref("css-url"), ref("css-url", "https://x.test/b")], false, "2 remote images blocked"],
    [[ref("stylesheet")], false, "1 remote stylesheet blocked"],
    [[ref("stylesheet"), ref("image")], false, "2 remote items blocked"],
    [[ref("image")], true, "Remote content loaded"],
  ])("summarises %j (loaded %s)", (refs, loaded, expected) => {
    expect(remoteSummary(refs, loaded)).toBe(expected);
  });

  it("labels kinds, types and counts", () => {
    expect(remoteKindLabel("css-url")).toBe("CSS image");
    expect(mimeLabel("application/pdf")).toBe("PDF");
    expect(mimeLabel("application/x-zip")).toBe("ZIP");
    expect(mimeLabel("binary")).toBe("BINARY");
    expect(attachmentsLabel(1)).toBe("1 attachment");
    expect(attachmentsLabel(2)).toBe("2 attachments");
  });
});
