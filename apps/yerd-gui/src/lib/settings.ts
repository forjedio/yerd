import type { AutostartState, StatusReport, UpdateStatusResponse } from "@/ipc/types";
import { humaniseAgo } from "@/lib/utils";

/** Pure derivations for the Settings screen and its About section. No I/O. */

/** Extract the port from a `host:port` socket address (e.g. `127.0.0.1:1053`). */
export function portFromAddr(addr: string | undefined): number {
  if (!addr) return 0;
  const port = Number(addr.slice(addr.lastIndexOf(":") + 1));
  return Number.isInteger(port) ? port : 0;
}

/** The update status line in the About section. */
export interface UpdateHeadline {
  tone: "ok" | "available" | "none";
  title: string;
  detail: string;
  /** The current / latest stable / latest edge figures, shown on hover. */
  tooltip: string | null;
}

/** The note for a build on a pre-release ahead of the stable channel. */
export const PRERELEASE_NOTE = "On a pre-release ahead of stable.";

/**
 * The About section's update headline. A status that was never checked (no
 * timestamp and no figures) reads as "Not checked yet", or asks for the daemon
 * when it is down. `now` (epoch seconds) pins the "Last checked" age in tests.
 */
export function updateHeadline(
  s: UpdateStatusResponse | null,
  opts: { running: boolean; applying: boolean; now?: number },
): UpdateHeadline {
  const none: UpdateHeadline = {
    tone: "none",
    title: opts.running ? "Not checked yet." : "Start the daemon to check for updates.",
    detail: "",
    tooltip: null,
  };
  if (!s) return none;
  const tooltip = `Current ${s.current} · stable ${s.latest_stable ?? "-"} · edge ${s.latest_edge ?? "-"}`;
  if (!s.checked_at_epoch && !s.latest_stable && !s.latest_edge) return { ...none, tooltip };
  if (s.available && s.target) {
    return {
      tone: "available",
      title: `Yerd ${s.target} is available`,
      detail: opts.applying
        ? `Updating to ${s.target}. Yerd will restart…`
        : `You're on ${s.current}. Yerd restarts to finish.`,
      tooltip,
    };
  }
  const checked = s.checked_at_epoch
    ? `Last checked ${humaniseAgo(s.checked_at_epoch, opts.now)}.`
    : "";
  const detail = s.ahead_of_stable ? `${PRERELEASE_NOTE}${checked ? ` ${checked}` : ""}` : checked;
  return { tone: "ok", title: "Up to date", detail, tooltip };
}

/** A warning shown at the top of the Settings column. */
export interface SettingsBanner {
  id: "daemon-approval" | "gui-approval" | "web-unbound" | "dns-unbound";
  tone: "warning" | "destructive";
  title: string;
  body: string;
  /** Whether the banner offers "Open Login Items". */
  loginItems: boolean;
}

/**
 * The Settings banners, in display order: the two macOS Login-Items approvals,
 * then the daemon's unbound web and DNS ports. Pass a null `report` while the
 * daemon is down so stale port warnings don't linger.
 */
export function settingsBanners(
  autostart: AutostartState | null,
  report: StatusReport | null,
): SettingsBanner[] {
  const out: SettingsBanner[] = [];
  if (autostart?.daemonPendingApproval) {
    out.push({
      id: "daemon-approval",
      tone: "warning",
      title: "Approve the Yerd background daemon",
      body: "Yerd is registered but waiting for you to enable it in System Settings › Login Items.",
      loginItems: true,
    });
  }
  if (autostart?.guiPendingApproval) {
    out.push({
      id: "gui-approval",
      tone: "warning",
      title: "Approve launching Yerd at login",
      body: "“Open the Yerd app at login” is set, but macOS needs you to enable it under System Settings › Login Items (Open at Login).",
      loginItems: true,
    });
  }
  const web = report?.web_unbound;
  if (web) {
    out.push({
      id: "web-unbound",
      tone: "destructive",
      title: "Yerd isn't serving any sites",
      body: `Ports ${web.http}/${web.https} are in use by another process. Pick free HTTP and HTTPS ports and save.`,
      loginItems: false,
    });
  }
  const dns = report?.dns_unbound;
  if (dns != null) {
    out.push({
      id: "dns-unbound",
      tone: "destructive",
      title: "Yerd can't resolve .test domains",
      body: `Port ${dns} is in use by another process. Pick a free DNS port and save. You may need to re-run Trust afterwards so the system points at the new port.`,
      loginItems: false,
    });
  }
  return out;
}
