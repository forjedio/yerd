/**
 * Typed front-end IPC client.
 *
 * Each function wraps a single Tauri command exposed by `src-tauri`
 * (commands.rs), which in turn performs exactly one `yerd-ipc` request against
 * the daemon. The bridge converts `Response::Error` into a rejected command, so
 * callers here only ever see the success variant or a thrown {@link IpcError}.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type {
  AvailablePhpResponse,
  Diagnosis,
  DoctorFixResponse,
  ElevateTarget,
  InfoResponse,
  PhpVersion,
  PhpVersionsResponse,
  Response,
  Site,
  StatusReport,
} from "./types";

/** A normalised IPC/host failure surfaced to the UI (toasts, banners). */
export class IpcError extends Error {
  /** Machine-readable category when the daemon supplied one. */
  readonly code: string;
  /** True when the daemon socket could not be reached at all. */
  readonly unreachable: boolean;

  constructor(message: string, code = "internal") {
    super(message);
    this.name = "IpcError";
    this.code = code;
    this.unreachable =
      code === "unreachable" || /daemon (is )?unreachable|not running/i.test(message);
  }
}

/** The bridge serialises its `GuiError` as `{ code, message }`. */
interface WireError {
  code?: string;
  message?: string;
}

function toIpcError(e: unknown): IpcError {
  if (e instanceof IpcError) return e;
  if (typeof e === "string") return new IpcError(e);
  if (e && typeof e === "object") {
    const w = e as WireError;
    if (w.message) return new IpcError(w.message, w.code ?? "internal");
  }
  return new IpcError(String(e));
}

/** Low-level invoke that normalises every rejection to {@link IpcError}. */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}

/** Defensive: if a `type:error` ever slips through, throw it like any failure. */
function ensureOk(r: Response): Response {
  if (r.type === "error") throw new IpcError(r.message, r.code);
  return r;
}

// ── daemon liveness ────────────────────────────────────────────────────────

export async function ping(): Promise<boolean> {
  const r = ensureOk(await call<Response>("ping"));
  return r.type === "pong";
}

// ── sites ──────────────────────────────────────────────────────────────────

export async function listSites(): Promise<Site[]> {
  const r = ensureOk(await call<Response>("list_sites"));
  return r.type === "sites" ? r.sites : [];
}

export async function park(path: string): Promise<void> {
  ensureOk(await call<Response>("park", { path }));
}

/** The registered parked roots, including empty ones (no derived sites). */
export async function listParked(): Promise<string[]> {
  const r = ensureOk(await call<Response>("list_parked"));
  return r.type === "parked" ? r.paths : [];
}

/**
 * Un-park a directory root: removes it from the parked set and re-scans. Pass a
 * path verbatim from {@link listParked} — the daemon matches it exactly (no
 * canonicalisation), so a folder deleted from disk is still removable.
 */
export async function unpark(path: string): Promise<void> {
  ensureOk(await call<Response>("unpark", { path }));
}

export async function link(name: string, path: string): Promise<void> {
  ensureOk(await call<Response>("link", { name, path }));
}

export async function unlink(name: string): Promise<void> {
  ensureOk(await call<Response>("unlink", { name }));
}

export async function setPhp(name: string, version: PhpVersion): Promise<void> {
  ensureOk(await call<Response>("set_php", { name, version }));
}

export async function setSecure(name: string, secure: boolean): Promise<void> {
  ensureOk(await call<Response>("set_secure", { name, secure }));
}

// ── php versions ───────────────────────────────────────────────────────────

export async function listPhp(): Promise<PhpVersionsResponse> {
  return ensureOk(await call<Response>("list_php")) as PhpVersionsResponse;
}

export async function checkPhpUpdates(): Promise<PhpVersionsResponse> {
  return ensureOk(await call<Response>("check_php_updates")) as PhpVersionsResponse;
}

/** Query the distribution for installable versions (+ what's already installed). */
export async function availablePhp(): Promise<AvailablePhpResponse> {
  return ensureOk(await call<Response>("available_php")) as AvailablePhpResponse;
}

export async function installPhp(version: PhpVersion): Promise<void> {
  ensureOk(await call<Response>("install_php", { version }));
}

export async function setDefaultPhp(version: PhpVersion): Promise<void> {
  ensureOk(await call<Response>("set_default_php", { version }));
}

/** `version === null` updates every installed version. */
export async function updatePhp(version: PhpVersion | null): Promise<void> {
  ensureOk(await call<Response>("update_php", { version }));
}

/** Restart one version's FPM pool (stop + start). */
export async function restartPhp(version: PhpVersion): Promise<void> {
  ensureOk(await call<Response>("restart_php", { version }));
}

/** Restart every started (running or failed) FPM pool. */
export async function restartAllPhp(): Promise<void> {
  ensureOk(await call<Response>("restart_all_php"));
}

/**
 * Restart the daemon process in place. The daemon replies and then re-execs, so
 * the connection drops momentarily; the status poll reconnects on its own.
 */
export async function restartDaemon(): Promise<void> {
  ensureOk(await call<Response>("restart_daemon"));
}

/**
 * Uninstall a PHP version. Rejects (toast-worthy) when the version is in use by
 * a site, is the last version with sites remaining, or is the current default.
 * Returns the refreshed version list.
 */
export async function uninstallPhp(version: PhpVersion): Promise<PhpVersionsResponse> {
  return ensureOk(
    await call<Response>("uninstall_php", { version }),
  ) as PhpVersionsResponse;
}

/**
 * Merge global PHP ini settings and apply them to every installed version's FPM
 * pool. An empty-string value resets a setting to PHP's default. Returns the
 * refreshed version list (which carries the applied settings).
 */
export async function setPhpSettings(
  settings: Record<string, string>,
): Promise<PhpVersionsResponse> {
  return ensureOk(
    await call<Response>("set_php_settings", { settings }),
  ) as PhpVersionsResponse;
}

// ── status / doctor ────────────────────────────────────────────────────────

export async function status(): Promise<StatusReport> {
  const r = ensureOk(await call<Response>("status"));
  if (r.type !== "status") throw new IpcError("unexpected response", "internal");
  return r.report;
}

export async function diagnose(): Promise<Diagnosis[]> {
  const r = ensureOk(await call<Response>("diagnose"));
  return r.type === "diagnoses" ? r.items : [];
}

export async function doctorFix(): Promise<DoctorFixResponse["report"]> {
  const r = ensureOk(await call<Response>("doctor_fix")) as DoctorFixResponse;
  return r.report;
}

// ── daemon info / about ────────────────────────────────────────────────────

export async function daemonInfo(): Promise<InfoResponse> {
  return ensureOk(await call<Response>("daemon_info")) as InfoResponse;
}

export async function protocolVersion(): Promise<number> {
  return call<number>("protocol_version");
}

/** Host OS, e.g. `"linux"` / `"macos"` / `"windows"` — gates platform UI. */
export async function hostPlatform(): Promise<string> {
  return call<string>("host_platform");
}

// ── host helpers (Tauri plugins, NOT daemon IPC) ───────────────────────────

export async function openInBrowser(url: string): Promise<void> {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

/** Reveal a file or directory in the OS file manager. */
export async function openPath(path: string): Promise<void> {
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/** Returns the chosen directory, or null if the user cancelled. */
export async function pickDirectory(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/**
 * Run `yerd elevate <target>` under OS elevation (pkexec / osascript). Returns
 * when the elevated process exits; rejects with the helper's message on failure.
 */
export async function elevate(target: ElevateTarget): Promise<void> {
  await call<void>("elevate", { target });
}
