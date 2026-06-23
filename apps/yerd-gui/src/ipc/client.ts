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
  AutostartState,
  AvailablePhpResponse,
  CreateSiteSpec,
  DatabaseSummary,
  Diagnosis,
  DoctorFixResponse,
  DumpCounts,
  DumpsResponse,
  DumpsStatusResponse,
  ElevateTarget,
  InfoResponse,
  JobProgressResponse,
  MailDetail,
  MailSummary,
  PhpVersion,
  PhpVersionsResponse,
  Response,
  ServiceAvailability,
  ServiceStatus,
  Site,
  StatusReport,
  ToolStatus,
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

/**
 * Set a site's served web root. `path` may be relative to the site folder
 * (e.g. "public") or an absolute path inside it; the daemon validates
 * containment and stores the relative remainder. Pass `null` to reset the site
 * to automatic framework detection.
 */
export async function setWebRoot(name: string, path: string | null): Promise<void> {
  ensureOk(await call<Response>("set_web_root", { name, path }));
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

// ── services (databases / caches) ────────────────────────────────────────────

export async function listServices(): Promise<ServiceStatus[]> {
  const r = ensureOk(await call<Response>("list_services"));
  return r.type === "services" ? r.services : [];
}

// ── dev tools (composer / node / bun) ────────────────────────────────────────

export async function listTools(): Promise<ToolStatus[]> {
  const r = ensureOk(await call<Response>("list_tools"));
  if (r.type !== "tools") throw new IpcError("unexpected response", "internal");
  return r.tools;
}

/** Install (or update to latest) a dev tool by id. Slow — downloads + verifies. */
export async function installTool(tool: string): Promise<void> {
  ensureOk(await call<Response>("install_tool", { tool }));
}

export async function uninstallTool(tool: string): Promise<void> {
  ensureOk(await call<Response>("uninstall_tool", { tool }));
}

/** Install a dev tool as a streamed job; returns the job id to poll. */
export async function installToolStreamed(tool: string): Promise<string> {
  const r = ensureOk(await call<Response>("install_tool_streamed", { tool }));
  if (r.type !== "job_started") throw new IpcError("unexpected response to install_tool_streamed");
  return r.job_id;
}

/**
 * Poll a job to completion, delivering each batch of new log lines via
 * `onLines`. Resolves with the latest {@link JobProgressResponse} — either the
 * terminal one, or the last seen when `shouldContinue()` returns false (e.g. the
 * viewing modal closed). `onLines` is the only log-delivery channel; the
 * returned `.log` is just the final batch.
 */
export async function pollJobToEnd(
  jobId: string,
  onLines: (lines: string[]) => void,
  shouldContinue?: () => boolean,
  intervalMs = 500,
): Promise<JobProgressResponse> {
  let cursor = 0;
  for (;;) {
    const r = await jobStatus(jobId, cursor);
    if (r.log.length) onLines(r.log);
    // Advance unconditionally (the daemon may report a phase change with no new
    // log lines), so this client never re-fetches a window or stalls.
    cursor = r.next_cursor;
    if (r.state !== "running") return r;
    if (shouldContinue && !shouldContinue()) return r;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

// ── site creation ──────────────────────────────────────────────────────────

/** Start scaffolding a new site; returns the job id to poll with {@link jobStatus}. */
export async function createSite(spec: CreateSiteSpec): Promise<string> {
  const r = ensureOk(await call<Response>("create_site", { spec }));
  if (r.type !== "job_started") throw new IpcError("unexpected response to create_site");
  return r.job_id;
}

/** Poll a job's progress. `cursor` is the number of log lines already held. */
export async function jobStatus(jobId: string, cursor: number): Promise<JobProgressResponse> {
  return ensureOk(await call<Response>("job_status", { jobId, cursor })) as JobProgressResponse;
}

/** Request cancellation of a running job. */
export async function jobCancel(jobId: string): Promise<void> {
  ensureOk(await call<Response>("job_cancel", { jobId }));
}

/** Installable vs installed versions per service. Fetches the listing on demand. */
export async function availableServices(): Promise<ServiceAvailability[]> {
  const r = ensureOk(await call<Response>("available_services"));
  return r.type === "available_services" ? r.services : [];
}

export async function installService(service: string, version: string): Promise<void> {
  ensureOk(await call<Response>("install_service", { service, version }));
}

export async function changeServiceVersion(service: string, version: string): Promise<void> {
  ensureOk(await call<Response>("change_service_version", { service, version }));
}

export async function uninstallService(
  service: string,
  version: string,
  purge: boolean,
): Promise<void> {
  ensureOk(await call<Response>("uninstall_service", { service, version, purge }));
}

export async function startService(service: string): Promise<void> {
  ensureOk(await call<Response>("start_service", { service }));
}

export async function stopService(service: string): Promise<void> {
  ensureOk(await call<Response>("stop_service", { service }));
}

export async function restartService(service: string): Promise<void> {
  ensureOk(await call<Response>("restart_service", { service }));
}

/** Persist a new port; takes effect on the next start/restart. */
export async function setServicePort(service: string, port: number): Promise<void> {
  ensureOk(await call<Response>("set_service_port", { service, port }));
}

/** The last `lines` lines of a service's log file. */
export async function serviceLogs(service: string, lines: number): Promise<string[]> {
  const r = ensureOk(await call<Response>("service_logs", { service, lines }));
  return r.type === "service_logs" ? r.lines : [];
}

export async function createDatabase(service: string, name: string): Promise<void> {
  ensureOk(await call<Response>("create_database", { service, name }));
}

/** The user databases in a running SQL service (system schemas filtered out). */
export async function listDatabases(service: string): Promise<DatabaseSummary[]> {
  const r = ensureOk(await call<Response>("list_databases", { service }));
  return r.type === "databases" ? r.databases : [];
}

export async function dropDatabase(service: string, name: string): Promise<void> {
  ensureOk(await call<Response>("drop_database", { service, name }));
}

/** Dump a database to a plain-SQL file (the daemon streams the bundled dump tool). */
export async function backupDatabase(
  service: string,
  name: string,
  path: string,
): Promise<void> {
  ensureOk(await call<Response>("backup_database", { service, name, path }));
}

/** Restore a database from a plain-SQL file (the database must already exist). */
export async function restoreDatabase(
  service: string,
  name: string,
  path: string,
): Promise<void> {
  ensureOk(await call<Response>("restore_database", { service, name, path }));
}

// ── mail capture ─────────────────────────────────────────────────────────────

/** Captured email metadata, newest first. */
export async function listMails(): Promise<MailSummary[]> {
  const r = ensureOk(await call<Response>("list_mails"));
  return r.type === "mails" ? r.mails : [];
}

/** One captured email's full decoded content (headers + bodies). */
export async function getMail(id: string): Promise<MailDetail> {
  const r = ensureOk(await call<Response>("get_mail", { id }));
  if (r.type !== "mail") throw new IpcError("unexpected response", "internal");
  return r.mail;
}

/** Delete every captured email. */
export async function clearMails(): Promise<void> {
  ensureOk(await call<Response>("clear_mails"));
}

/** Delete a specific set of captured emails by id (e.g. one application's mail). */
export async function deleteMails(ids: string[]): Promise<void> {
  ensureOk(await call<Response>("delete_mails", { ids }));
}

/** Persist the mail-capture SMTP port; takes effect on the next daemon restart. */
export async function setMailPort(port: number): Promise<void> {
  ensureOk(await call<Response>("set_mail_port", { port }));
}

/** Enable/disable mail capture; takes effect on the next daemon restart. */
export async function setMailEnabled(enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_mail_enabled", { enabled }));
}

/** Open (or focus) the separate Mails viewer window. Host command, not daemon IPC. */
export async function showMailsWindow(): Promise<void> {
  await call<void>("show_mails_window");
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

/** Open a file with the OS default app for its type (e.g. the user's editor). */
export async function openInEditor(path: string): Promise<void> {
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** Returns the chosen directory, or null if the user cancelled. */
export async function pickDirectory(defaultPath?: string): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false, defaultPath });
  return typeof picked === "string" ? picked : null;
}

/** Save-file dialog (for backups). Returns the chosen path, or null if cancelled. */
export async function pickSaveFile(defaultPath?: string): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ defaultPath });
  return typeof picked === "string" ? picked : null;
}

/** Open-file dialog (for restores). Returns the chosen path, or null if cancelled. */
export async function pickOpenFile(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: false, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/**
 * Run `yerd elevate <target>` under OS elevation (pkexec / osascript). Returns
 * when the elevated process exits; rejects with the helper's message on failure.
 */
export async function elevate(target: ElevateTarget): Promise<void> {
  await call<void>("elevate", { target });
}

/**
 * Run `yerd elevate` with no target under OS elevation — applies every step
 * (trust, resolver, ports) in one prompt.
 */
export async function elevateAll(): Promise<void> {
  await call<void>("elevate_all");
}

/**
 * Revert `elevate` for `target` under the same OS elevation (`yerd unelevate
 * <target>`). On macOS, unelevating the resolver restores the pre-Yerd resolver
 * from its backup (or removes Yerd's file if none). `ports` is reversible on
 * macOS only — callers gate the button accordingly.
 */
export async function unelevate(target: ElevateTarget): Promise<void> {
  await call<void>("unelevate", { target });
}

/**
 * Trust the local CA for the current user, in-process (macOS only). Needs no
 * root and prompts as "Yerd". Rejects with the OS error message on failure
 * (e.g. cancelled, keychain locked).
 */
export async function trustCa(): Promise<void> {
  await call<void>("trust_ca");
}

/**
 * Remove the current user's trust of the local CA (macOS only). Resolves to
 * `true` if the CA is *still* trusted afterwards — i.e. a system-wide trust set
 * via the terminal remains, which the GUI can't remove without root.
 */
export async function untrustCa(): Promise<boolean> {
  return call<boolean>("untrust_ca");
}

// ── daemon lifecycle + autostart (host commands, NOT daemon IPC) ────────────

/** Is the `yerdd` binary installed on disk (independent of whether it's running)? */
export async function daemonInstalled(): Promise<boolean> {
  return call<boolean>("daemon_installed");
}

/** Download + install the matching `yerdd` release. Emits `install-progress` events. */
export async function installDaemon(): Promise<void> {
  await call<void>("install_daemon");
}

export async function startDaemon(): Promise<void> {
  await call<void>("start_daemon");
}

export async function stopDaemon(): Promise<void> {
  await call<void>("stop_daemon");
}

export async function getAutostart(): Promise<AutostartState> {
  return call<AutostartState>("get_autostart");
}

export async function setAutostartDaemon(on: boolean): Promise<void> {
  await call<void>("set_autostart_daemon", { on });
}

export async function setAutostartGui(on: boolean): Promise<void> {
  await call<void>("set_autostart_gui", { on });
}

export async function setAutostartGuiMinimized(on: boolean): Promise<void> {
  await call<void>("set_gui_minimized", { on });
}

/** Subscribe to `yerdd` install-progress messages. Returns an unlisten fn. */
export async function onInstallProgress(
  cb: (message: string) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<string>("install-progress", (e) => cb(e.payload));
}

// ── dumps (Laravel telemetry) ────────────────────────────────────────────────

/** Page buffered dump events newer than `since` (0 = all). */
export async function listDumps(since: number): Promise<DumpsResponse> {
  const r = ensureOk(await call<Response>("list_dumps", { since }));
  if (r.type === "dumps") return r;
  return {
    type: "dumps",
    events: [],
    removed_ids: [],
    counts: emptyDumpCounts(),
    latest_id: 0,
    min_live_id: 0,
  };
}

/** Dump-server status (enabled, port, running, extension presence, counts). */
export async function dumpsStatus(): Promise<DumpsStatusResponse> {
  const r = ensureOk(await call<Response>("dumps_status"));
  if (r.type === "dumps_status") return r;
  return {
    type: "dumps_status",
    enabled: false,
    port: 2304,
    running: false,
    persist: false,
    extensions: [],
    counts: emptyDumpCounts(),
    features: {},
  };
}

export async function clearDumps(): Promise<void> {
  ensureOk(await call<Response>("clear_dumps"));
}

export async function deleteDump(id: number): Promise<void> {
  ensureOk(await call<Response>("delete_dump", { id }));
}

export async function setDumpsEnabled(enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_dumps_enabled", { enabled }));
}

export async function setDumpsPersist(persist: boolean): Promise<void> {
  ensureOk(await call<Response>("set_dumps_persist", { persist }));
}

export async function setDumpsPort(port: number): Promise<void> {
  ensureOk(await call<Response>("set_dumps_port", { port }));
}

export async function setDumpFeature(feature: string, enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_dump_feature", { feature, enabled }));
}

/** Open the standalone dumps viewer window. */
export async function showDumpsWindow(): Promise<void> {
  await call<void>("show_dumps_window");
}

function emptyDumpCounts(): DumpCounts {
  return { dumps: 0, queries: 0, jobs: 0, views: 0, requests: 0, logs: 0, cache: 0, http: 0 };
}
