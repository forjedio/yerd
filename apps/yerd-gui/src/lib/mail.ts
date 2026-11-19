/**
 * Pure derivations for the Mail screen and the Mails window. No I/O; times
 * take `now` (epoch seconds) as an argument.
 */
import type { MailStatus, MailSummary } from "@/ipc/types";
import type { RemoteContentKind, RemoteContentRef } from "@/lib/mailLinks";
import { plural } from "@/lib/utils";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAY_MS = 86_400_000;

/** A sender split into its display name and address. */
export interface SenderParts {
  name: string;
  address: string;
}

/** Split `Name <addr>` (quotes stripped) or a bare address; a bare address has no name. */
export function senderParts(from: string): SenderParts {
  const named = from.match(/^\s*(.*?)\s*<([^>]+)>\s*$/);
  if (named) {
    return { name: (named[1] ?? "").replace(/^"|"$/g, "").trim(), address: (named[2] ?? "").trim() };
  }
  return { name: "", address: from.trim() };
}

/** The app a mail is grouped under: the sender's display name, else the
 *  attributed site, else the bare address. */
export function mailApp(m: MailSummary): string {
  const sender = senderParts(m.from);
  return sender.name || m.site || sender.address || "(unknown sender)";
}

/** One entry of the app filter menu. */
export interface AppOption {
  value: string;
  label: string;
  count: number;
}

/** The apps in the mailbox with their counts, sorted by name. An active filter
 *  whose mail is gone stays listed with a zero count. */
export function appOptions(list: readonly MailSummary[], selected: string): AppOption[] {
  const counts = new Map<string, number>();
  for (const m of list) {
    const app = mailApp(m);
    counts.set(app, (counts.get(app) ?? 0) + 1);
  }
  const options = [...counts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([value, count]) => ({ value, label: value, count }));
  if (selected && !counts.has(selected)) options.push({ value: selected, label: selected, count: 0 });
  return options;
}

/** The mails of one app; `""` keeps them all. */
export function filterByApp(list: readonly MailSummary[], app: string): MailSummary[] {
  return app ? list.filter((m) => mailApp(m) === app) : [...list];
}

/** Mails explicitly marked unread (an older daemon omits `read`, counted as read). */
export function unreadCount(list: readonly MailSummary[]): number {
  return list.filter((m) => m.read === false).length;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

function clock(d: Date): string {
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function daysBefore(then: Date, now: Date): number {
  const a = new Date(then.getFullYear(), then.getMonth(), then.getDate()).getTime();
  const b = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  return Math.round((b - a) / DAY_MS);
}

function dayMonth(d: Date): string {
  return `${d.getDate()} ${MONTHS[d.getMonth()] ?? ""}`;
}

type DayKind = "today" | "yesterday" | "week" | "year" | "older";

function dayKind(then: Date, now: Date): DayKind {
  const days = daysBefore(then, now);
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  if (days > 1 && days <= 6) return "week";
  return then.getFullYear() === now.getFullYear() ? "year" : "older";
}

/** A list row's time: `14:32`, `Yesterday`, `Mon`, `4 Nov` or `4 Nov 2025`; `-` without a date. */
export function listTime(epoch: number, now: number): string {
  if (!epoch) return "-";
  const then = new Date(epoch * 1000);
  const kind = dayKind(then, new Date(now * 1000));
  if (kind === "today") return clock(then);
  if (kind === "yesterday") return "Yesterday";
  if (kind === "week") return WEEKDAYS[then.getDay()] ?? "";
  if (kind === "year") return dayMonth(then);
  return `${dayMonth(then)} ${then.getFullYear()}`;
}

/** The reader's time: `Today 14:32`, `Yesterday 09:10`, `Mon 11:48`, `4 Nov 11:48`
 *  or `4 Nov 2025, 11:48`; `-` without a date. */
export function readerTime(epoch: number, now: number): string {
  if (!epoch) return "-";
  const then = new Date(epoch * 1000);
  const kind = dayKind(then, new Date(now * 1000));
  if (kind === "today") return `Today ${clock(then)}`;
  if (kind === "yesterday") return `Yesterday ${clock(then)}`;
  if (kind === "week") return `${WEEKDAYS[then.getDay()] ?? ""} ${clock(then)}`;
  if (kind === "year") return `${dayMonth(then)} ${clock(then)}`;
  return `${dayMonth(then)} ${then.getFullYear()}, ${clock(then)}`;
}

/** The toolbar's capture pill. */
export interface CaptureState {
  tone: "ok" | "warn" | "off" | "pending";
  label: string;
  title: string;
}

/**
 * The capture pill from the daemon's mail status. The listener binds only at
 * startup, so after a toggle in the current daemon boot (`pendingRestart`) the
 * config and the listener disagree until Yerd restarts; that reads as
 * "Restart Yerd to apply" rather than a misleading "Port in use".
 */
export function captureStatus(mail: MailStatus | null, pendingRestart: boolean): CaptureState {
  if (!mail) return { tone: "off", label: "Capture unknown", title: "Yerd's mail status isn't available." };
  if (pendingRestart && mail.enabled !== mail.listening) {
    return {
      tone: "pending",
      label: "Restart Yerd to apply",
      title: `Capture was turned ${mail.enabled ? "on" : "off"}; it takes effect when Yerd restarts.`,
    };
  }
  if (mail.enabled && mail.listening) {
    return { tone: "ok", label: `Capturing on ${mail.port}`, title: `Capturing mail sent to 127.0.0.1:${mail.port}.` };
  }
  if (mail.enabled) {
    return {
      tone: "warn",
      label: `Port ${mail.port} in use`,
      title: `Capture is on but port ${mail.port} was busy when Yerd started. Pick another in Settings ▸ Application Ports.`,
    };
  }
  return { tone: "off", label: "Capture off", title: "Mail capture is turned off." };
}

/** Inputs for {@link laravelEnv}. */
export interface LaravelEnvInput {
  port: number;
  fromName: string;
  fromAddress: string;
}

function envQuoted(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

/** The Laravel `.env` lines that route an app's mail through Yerd's capture
 *  server. The From name defaults to Laravel's `${APP_NAME}` idiom; a typed
 *  backslash or double quote is escaped so the quoted value stays valid. */
export function laravelEnv({ port, fromName, fromAddress }: LaravelEnvInput): string {
  const name = envQuoted(fromName.trim()) || "${APP_NAME}";
  const address = envQuoted(fromAddress.trim()) || "hello@example.com";
  return [
    "MAIL_MAILER=smtp",
    "MAIL_HOST=127.0.0.1",
    `MAIL_PORT=${port}`,
    "MAIL_USERNAME=null",
    "MAIL_PASSWORD=null",
    "MAIL_ENCRYPTION=null",
    `MAIL_FROM_ADDRESS="${address}"`,
    `MAIL_FROM_NAME="${name}"`,
  ].join("\n");
}

/** The reader's remote-content bar text, or `null` when the mail has none. */
export function remoteSummary(refs: readonly RemoteContentRef[], loaded: boolean): string | null {
  if (refs.length === 0) return null;
  if (loaded) return "Remote content loaded";
  const n = refs.length;
  if (refs.every((r) => r.kind !== "stylesheet")) return `${plural(n, "remote image", "remote images")} blocked`;
  if (refs.every((r) => r.kind === "stylesheet")) {
    return `${plural(n, "remote stylesheet", "remote stylesheets")} blocked`;
  }
  return `${n} remote items blocked`;
}

/** A remote reference's kind as shown in the Headers tab. */
export function remoteKindLabel(kind: RemoteContentKind): string {
  if (kind === "stylesheet") return "Stylesheet";
  if (kind === "css-url") return "CSS image";
  return "Image";
}

/** A short file-type label from a MIME type, e.g. `application/pdf` to `PDF`. */
export function mimeLabel(contentType: string): string {
  const sub = contentType.split("/")[1] ?? contentType;
  return sub.replace(/^x-/, "").toUpperCase();
}

/** `1 attachment` or `{n} attachments`. */
export function attachmentsLabel(n: number): string {
  return plural(n, "attachment", "attachments");
}
