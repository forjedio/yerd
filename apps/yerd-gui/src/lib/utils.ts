import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

import type { PoolRunState } from "@/ipc/types";

/** shadcn-vue's `cn`: merge conditional class lists, de-duping Tailwind utils. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

/** Status-dot tones (mirrors StatusPill's `Tone`). */
export type StatusTone = "ok" | "warn" | "bad" | "unknown" | "muted";

/**
 * Human label for an FPM pool's run state. PHP-FPM is started **on demand** when
 * a site first uses a version, so an installed-but-not-serving version is
 * `stopped` on the wire - which reads as alarming. Show it as "idle" instead;
 * reserve "failed" (red) for a pool that actually crashed. A version not yet in
 * the status report (e.g. just installed, before the next poll) is also "idle"
 * rather than a transient "not started".
 */
export function poolStateLabel(state: PoolRunState | null | undefined): string {
  switch (state) {
    case "running":
      return "running";
    case "failed":
      return "failed";
    default:
      return "idle"; // stopped / not-yet-reported → idle, ready on demand
  }
}

export function poolStateTone(state: PoolRunState | null | undefined): StatusTone {
  switch (state) {
    case "running":
      return "ok";
    case "failed":
      return "bad";
    default:
      return "muted"; // stopped / unknown → idle, neutral
  }
}

/** Humanise a duration given in whole seconds (e.g. `90061` -> `1d 1h 1m`). */
export function humaniseUptime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return "-";
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3_600);
  const m = Math.floor((secs % 3_600) / 60);
  const s = secs % 60;
  const parts: string[] = [];
  if (d) parts.push(`${d}d`);
  if (h) parts.push(`${h}h`);
  if (m) parts.push(`${m}m`);
  if (!d && !h) parts.push(`${s}s`);
  return parts.join(" ");
}

/** Render a past Unix-epoch (seconds) as a coarse, single-unit "… ago" string
 *  (e.g. "5 minutes ago", "2 hours ago", "3 days ago"). */
export function humaniseAgo(epochSecs: number, nowSecs: number = Date.now() / 1000): string {
  const diff = Math.floor(nowSecs - epochSecs);
  if (!Number.isFinite(diff) || diff < 45) return "just now";
  if (diff < 90) return "a minute ago";
  const mins = Math.round(diff / 60);
  if (mins < 60) return `${mins} minutes ago`;
  const hours = Math.round(diff / 3_600);
  if (hours < 24) return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  const days = Math.round(diff / 86_400);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

/** Render bytes as a short human string (e.g. `1536` -> `1.5 MB` base-2). */
export function humaniseBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "-";
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${u === 0 ? v : v.toFixed(1)} ${units[u]}`;
}

/**
 * Render the daemon's `load_avg` (each value is load × 100, i.e. hundredths -
 * see yerd-ipc status.rs) back to the conventional `x.xx` triple.
 */
export function formatLoadAvg(load: [number, number, number] | null): string {
  if (!load) return "-";
  return load.map((h) => (h / 100).toFixed(2)).join("  ");
}
