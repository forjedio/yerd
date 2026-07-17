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
  CliPathStatus,
  AvailablePhpResponse,
  CreateSiteSpec,
  DaemonDiagnostics,
  DatabaseSummary,
  Diagnosis,
  DoctorFixResponse,
  DumpCounts,
  DumpsResponse,
  DumpsStatusResponse,
  ElevateTarget,
  GroupsState,
  GuiLogs,
  InfoResponse,
  JobProgressResponse,
  MailDetail,
  MailSummary,
  AddableServiceType,
  PhpExtInfo,
  PhpVersion,
  PhpVersionsResponse,
  ProxyEntry,
  ProxyRuleEntry,
  Response,
  ServiceAvailability,
  ServiceStatus,
  SetupState,
  SiteEntry,
  NamedTunnelsResponse,
  StatusReport,
  TitleBarStyle,
  ToolStatus,
  TrayIconVariant,
  TunnelsResponse,
  UpdateChannel,
  UpdateStatusResponse,
  WordPressAdminUser,
  WordPressVersionInfo,
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

export async function listSites(): Promise<SiteEntry[]> {
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

/** Combined sites + parked roots + groups, the shape backing the shared
 * `"sites"` cached resource. One definition so the Sites view, the Overview, and
 * the command palette all key the same cache and can never drift apart. The
 * `groups` leg is fault-tolerant: a GUI newer than its daemon (an unknown
 * `list_groups` request before the bundled daemon restarts) must not reject the
 * whole `Promise.all` and take down the Overview / command palette, which never
 * needed groups - so it degrades to empty. */
export async function sitesAndParked(): Promise<{
  sites: SiteEntry[];
  parked: string[];
  groups: GroupsState;
}> {
  const [sites, parked, groups] = await Promise.all([
    listSites(),
    listParked(),
    listGroups().catch(() => ({ order: [], members: {} }) as GroupsState),
  ]);
  return { sites, parked, groups };
}

// ── site groups ──────────────────────────────────────────────────────────────

/** The user-defined groups (ordered) and per-site membership. */
export async function listGroups(): Promise<GroupsState> {
  const r = ensureOk(await call<Response>("list_groups"));
  return r.type === "groups" ? { order: r.order, members: r.members } : { order: [], members: {} };
}

/** Create a new group (appended last). Rejects empty/duplicate/reserved names. */
export async function createGroup(name: string): Promise<void> {
  ensureOk(await call<Response>("create_group", { name }));
}

/** Delete a group; its sites fall back to "Unallocated". */
export async function deleteGroup(name: string): Promise<void> {
  ensureOk(await call<Response>("delete_group", { name }));
}

/** Replace the group display order (must be a permutation of existing groups). */
export async function setGroupOrder(order: string[]): Promise<void> {
  ensureOk(await call<Response>("set_group_order", { order }));
}

/** Rename a group, preserving its position and moving its members. */
export async function renameGroup(from: string, to: string): Promise<void> {
  ensureOk(await call<Response>("rename_group", { from, to }));
}

/** Set or clear a site's group (`null` = Unallocated). */
export async function setSiteGroup(site: string, group: string | null): Promise<void> {
  ensureOk(await call<Response>("set_site_group", { site, group }));
}

/**
 * Un-park a directory root: removes it from the parked set and re-scans. Pass a
 * path verbatim from {@link listParked} - the daemon matches it exactly (no
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

// ── domains ──────────────────────────────────────────────────────────────────

/** Add a routable domain (exact or `*.`-wildcard, a full FQDN) to a site. The
 *  daemon validates shape + TLD membership and rejects a domain already claimed
 *  by another site. */
export async function addDomain(name: string, domain: string): Promise<void> {
  ensureOk(await call<Response>("add_domain", { name, domain }));
}

/** Remove a domain from a site. The daemon rejects removing the last exact
 *  (non-wildcard) domain. */
export async function removeDomain(name: string, domain: string): Promise<void> {
  ensureOk(await call<Response>("remove_domain", { name, domain }));
}

/** Set a site's primary (canonical) domain; auto-adds it if absent. Must be an
 *  exact domain - the daemon rejects a wildcard. For a WordPress site this also
 *  rewrites its `siteurl`/`home`. */
export async function setPrimaryDomain(name: string, domain: string): Promise<void> {
  ensureOk(await call<Response>("set_primary_domain", { name, domain }));
}

/** Clear a site's domain customization, returning it to the default apex-only
 *  `{name}.{tld}`. */
export async function resetDomains(name: string): Promise<void> {
  ensureOk(await call<Response>("reset_domains", { name }));
}

// ── proxies ──────────────────────────────────────────────────────────────────

/** Whole-host proxies plus per-site path-prefix rules. HTTPS on a whole-host
 *  proxy is toggled via {@link setSecure} (the daemon handles proxies there). */
export async function listProxies(): Promise<{ proxies: ProxyEntry[]; rules: ProxyRuleEntry[] }> {
  const r = ensureOk(await call<Response>("list_proxies"));
  return r.type === "proxies" ? { proxies: r.proxies, rules: r.rules } : { proxies: [], rules: [] };
}

/** Register a whole-host reverse proxy (`{name}.{tld}` → `url`). The daemon
 *  validates the name + URL and rejects collisions / `.test` loop targets. New
 *  proxies start over HTTP; enable HTTPS with {@link setSecure}. */
export async function addProxy(name: string, url: string): Promise<void> {
  ensureOk(await call<Response>("add_proxy", { name, url }));
}

export async function removeProxy(name: string): Promise<void> {
  ensureOk(await call<Response>("remove_proxy", { name }));
}

/** Add a path-prefix rule to an existing site (`site.test/prefix` → `url`),
 *  leaving every other path served by PHP. */
export async function addProxyRule(site: string, prefix: string, url: string): Promise<void> {
  ensureOk(await call<Response>("add_proxy_rule", { site, prefix, url }));
}

export async function removeProxyRule(site: string, prefix: string): Promise<void> {
  ensureOk(await call<Response>("remove_proxy_rule", { site, prefix }));
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

/** Start a streamed PHP install as a background job; returns the job id to poll. */
export async function installPhpStreamed(version: PhpVersion): Promise<string> {
  const r = ensureOk(await call<Response>("install_php_streamed", { version }));
  if (r.type !== "job_started") throw new IpcError("unexpected response to install_php_streamed");
  return r.job_id;
}

/**
 * Install a PHP version as a streamed job, delivering progress lines via
 * `onProgress`, and resolving only when it finishes. Throws (toast-worthy) on a
 * failed/cancelled job, so callers keep their existing try/catch around a
 * single awaited install.
 */
export async function installPhpWithProgress(
  version: PhpVersion,
  onProgress?: (lines: string[]) => void,
): Promise<void> {
  const jobId = await installPhpStreamed(version);
  const final = await pollJobToEnd(jobId, (lines) => onProgress?.(lines));
  if (final.state !== "succeeded") {
    throw new IpcError(final.error || `PHP ${version} install ${final.state}`);
  }
}

export async function setDefaultPhp(version: PhpVersion): Promise<void> {
  ensureOk(await call<Response>("set_default_php", { version }));
}

/** `version === null` updates every installed version. */
export async function updatePhp(version: PhpVersion | null): Promise<void> {
  ensureOk(await call<Response>("update_php", { version }));
}

// ── self-update ────────────────────────────────────────────────────────────

/**
 * Check for a Yerd self-update. `channel` overrides the saved preference for
 * this check only; omit (undefined) to use the saved default. Tolerant of
 * network failure - the daemon serves its cache (`source: "cached"`).
 */
export async function checkUpdates(channel?: UpdateChannel): Promise<UpdateStatusResponse> {
  return ensureOk(
    await call<Response>("check_updates", { channel: channel ?? null }),
  ) as UpdateStatusResponse;
}

/** Last persisted update-check result (no network) - pre-fills the UI on load. */
export async function cachedUpdateStatus(): Promise<UpdateStatusResponse> {
  return ensureOk(await call<Response>("cached_update_status")) as UpdateStatusResponse;
}

/** Persist the self-update channel preference. */
export async function setUpdateChannel(channel: UpdateChannel): Promise<void> {
  ensureOk(await call<Response>("set_update_channel", { channel }));
}

/**
 * Download + verify the latest update and launch the applier. The GUI quits so
 * its bundle can be swapped; the applier relaunches it when done. `channel`
 * overrides the saved preference for this apply only. The returned promise
 * typically never resolves (the app exits) - callers should show an "updating"
 * state before awaiting and treat a thrown error as a staging failure.
 */
export async function applyUpdate(channel?: UpdateChannel): Promise<void> {
  await call<void>("apply_update", { channel: channel ?? null });
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

/**
 * Merge per-version overrides of the allowlisted settings for one installed
 * version and apply them to that version's FPM pool + CLI ini. An empty-string
 * value removes the override (the global default applies again). Returns the
 * refreshed version list.
 */
export async function setPhpVersionSettings(
  version: PhpVersion,
  settings: Record<string, string>,
): Promise<PhpVersionsResponse> {
  return ensureOk(
    await call<Response>("set_php_version_settings", { version, settings }),
  ) as PhpVersionsResponse;
}

// ── php extensions ───────────────────────────────────────────────────────────

/** Registered custom extensions, keyed by version string (e.g. `"8.5"`). */
export type PhpExtensionsMap = Record<PhpVersion, PhpExtInfo[]>;

function extensionsOf(r: Response): PhpExtensionsMap {
  return r.type === "php_extensions" ? r.by_version : {};
}

export async function listPhpExtensions(): Promise<PhpExtensionsMap> {
  return extensionsOf(ensureOk(await call<Response>("list_php_extensions")));
}

/**
 * Register a custom extension for a version. The daemon load-probes the `.so`
 * first; a failure surfaces as a rejected command. Returns the refreshed map.
 */
export async function addPhpExtension(
  version: PhpVersion,
  path: string,
  zend: boolean,
  name?: string,
): Promise<PhpExtensionsMap> {
  return extensionsOf(
    ensureOk(
      await call<Response>("add_php_extension", {
        version,
        path,
        name: name ?? null,
        zend,
      }),
    ),
  );
}

export async function removePhpExtension(
  version: PhpVersion,
  name: string,
): Promise<PhpExtensionsMap> {
  return extensionsOf(
    ensureOk(await call<Response>("remove_php_extension", { version, name })),
  );
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

/** Install (or update to latest) a dev tool by id. Slow - downloads + verifies. */
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
 * `onLines`. Resolves with the latest {@link JobProgressResponse} - either the
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

// ── tunnels (Cloudflare Tunnel integration) ──────────────────────────────────

/** Install the `cloudflared` binary as a streamed job; returns the job id to poll. */
export async function installCloudflaredStreamed(): Promise<string> {
  const r = ensureOk(await call<Response>("install_cloudflared_streamed"));
  if (r.type !== "job_started")
    throw new IpcError("unexpected response to install_cloudflared_streamed");
  return r.job_id;
}

/** Share a site via a Quick Tunnel; returns the refreshed tunnel list. */
export async function startQuickTunnel(site: string): Promise<TunnelsResponse> {
  return ensureOk(await call<Response>("start_quick_tunnel", { site })) as TunnelsResponse;
}

/** Stop a site's tunnel; returns the refreshed tunnel list. */
export async function stopTunnel(site: string): Promise<TunnelsResponse> {
  return ensureOk(await call<Response>("stop_tunnel", { site })) as TunnelsResponse;
}

/** Live tunnels plus `cloudflared` install status. */
export async function tunnelStatus(): Promise<TunnelsResponse> {
  return ensureOk(await call<Response>("tunnel_status")) as TunnelsResponse;
}

/** Start the Cloudflare account login as a streamed job; returns the job id. */
export async function cloudflaredLogin(): Promise<string> {
  const r = ensureOk(await call<Response>("cloudflared_login"));
  if (r.type !== "job_started") throw new IpcError("unexpected response to cloudflared_login");
  return r.job_id;
}

/** Create a named tunnel on the logged-in account. */
export async function createNamedTunnel(name: string): Promise<void> {
  ensureOk(await call<Response>("create_named_tunnel", { name }));
}

/** Delete a named tunnel from the account and forget it locally. */
export async function deleteNamedTunnel(name: string): Promise<void> {
  ensureOk(await call<Response>("delete_named_tunnel", { name }));
}

/** The named tunnels recorded locally. */
export async function listNamedTunnels(): Promise<NamedTunnelsResponse> {
  return ensureOk(await call<Response>("list_named_tunnels")) as NamedTunnelsResponse;
}

/** Route a public hostname to a named tunnel (creates the DNS record). */
export async function routeTunnelDns(tunnel: string, hostname: string): Promise<void> {
  ensureOk(await call<Response>("route_tunnel_dns", { tunnel, hostname }));
}

/** Set (or clear, with `null`) a site's persisted public hostname. */
export async function setSiteTunnel(site: string, hostname: string | null): Promise<void> {
  ensureOk(await call<Response>("set_site_tunnel", { site, hostname }));
}

/** (Re)start the consolidated Named Tunnel serving all enabled sites. */
export async function startNamedTunnel(): Promise<TunnelsResponse> {
  return ensureOk(await call<Response>("start_named_tunnel")) as TunnelsResponse;
}

/** Stop the consolidated Named Tunnel. */
export async function stopNamedTunnel(): Promise<TunnelsResponse> {
  return ensureOk(await call<Response>("stop_named_tunnel")) as TunnelsResponse;
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

/** WordPress core version branches with their PHP compatibility range, from
 *  the yerd repo's hand-maintained meta/wordpress-versions.json. Daemon-cached;
 *  see `available_services` for the equivalent for services. */
export async function availableWordPressVersions(): Promise<WordPressVersionInfo[]> {
  const r = ensureOk(await call<Response>("available_wordpress_versions"));
  return r.type === "wordpress_versions" ? r.versions : [];
}

/** Mint a short-TTL, single-use token for one-click, pre-authenticated
 *  WordPress admin login (the "WP Admin" site action). Rejects if `site`
 *  doesn't exist or isn't WordPress. */
export async function mintWordPressLoginToken(site: string): Promise<string> {
  const r = ensureOk(await call<Response>("mint_wordpress_login_token", { site }));
  if (r.type !== "wordpress_login_token") {
    throw new IpcError("unexpected response to mint_wordpress_login_token");
  }
  return r.token;
}

/** Toggle WordPress one-click admin login for a site, and set which admin
 *  user it signs in as. Pass `user: null` to fall back to the
 *  earliest-created administrator. */
export async function setWordpressAutoLogin(
  name: string,
  enabled: boolean,
  user: string | null,
): Promise<void> {
  ensureOk(await call<Response>("set_wordpress_auto_login", { name, enabled, user }));
}

/** Override a site's front-controller mode: `true` funnels every request through
 *  the site-root `index.php`; `false` executes named `.php` files directly. */
export async function setFrontController(name: string, enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_front_controller", { name, enabled }));
}

/** List a WordPress site's administrator accounts, for the auto-login user
 *  picker. Fetched on demand via `wp user list`. */
export async function wordpressAdminUsers(site: string): Promise<WordPressAdminUser[]> {
  const r = ensureOk(await call<Response>("wordpress_admin_users", { site }));
  return r.type === "wordpress_admin_users" ? r.users : [];
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

/** The installable service types for the "Add Service" dialog. */
export async function addableServiceTypes(): Promise<AddableServiceType[]> {
  const r = ensureOk(await call<Response>("addable_service_types"));
  return r.type === "addable_services" ? r.types : [];
}

/** Add a new service instance. Returns its wire id. Tauri maps camelCase JS args
 *  to the command's snake_case Rust params, so `type_id` is sent as `typeId`. */
export async function addService(args: {
  type_id: string;
  site: string | null;
  port: number | null;
  version: string | null;
  autostart: boolean;
}): Promise<string> {
  const r = ensureOk(
    await call<Response>("add_service", {
      typeId: args.type_id,
      site: args.site,
      port: args.port,
      version: args.version,
      autostart: args.autostart,
    }),
  );
  return r.type === "service_instance_id" ? r.id : "";
}

/** Remove a per-site service instance. */
export async function removeService(service: string, purge: boolean): Promise<void> {
  ensureOk(await call<Response>("remove_service", { service, purge }));
}

/** Set whether a service starts with Yerd. */
export async function setServiceAutostart(service: string, enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_service_autostart", { service, enabled }));
}

/** Re-link a per-site instance to a different site. Returns the new wire id. */
export async function setServiceSite(service: string, site: string): Promise<string> {
  const r = ensureOk(await call<Response>("set_service_site", { service, site }));
  return r.type === "service_instance_id" ? r.id : service;
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

/** Mark a specific set of captured emails as read by id. */
export async function markMailsRead(ids: string[]): Promise<void> {
  ensureOk(await call<Response>("mark_mails_read", { ids }));
}

/** Persist the mail-capture SMTP port; takes effect on the next daemon restart. */
export async function setMailPort(port: number): Promise<void> {
  ensureOk(await call<Response>("set_mail_port", { port }));
}

/**
 * Persist the rootless HTTP/HTTPS fallback ports; takes effect on the next
 * daemon restart. The daemon rejects values < 1024, equal ports, or a change
 * while ports are elevated (surfaced as a thrown IpcError).
 */
export async function setFallbackPorts(http: number, https: number): Promise<void> {
  ensureOk(await call<Response>("set_fallback_ports", { http, https }));
}

/** Set the embedded DNS responder port; takes effect on the next daemon restart. */
export async function setDnsPort(port: number): Promise<void> {
  ensureOk(await call<Response>("set_dns_port", { port }));
}

/** Enable/disable mail capture; takes effect on the next daemon restart. */
export async function setMailEnabled(enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_mail_enabled", { enabled }));
}

/** Enable/disable the proxy's symlink-escape protection; takes effect immediately. */
export async function setSymlinkProtection(enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_symlink_protection", { enabled }));
}

/** Enable/disable serving Yerd's tools to AI agents over MCP. Enabling reaches
 *  running agent sessions on their next tool call; disabling applies to sessions
 *  started afterwards. */
export async function setMcpEnabled(enabled: boolean): Promise<void> {
  ensureOk(await call<Response>("set_mcp_enabled", { enabled }));
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

/** Host OS, e.g. `"linux"` / `"macos"` / `"windows"` - gates platform UI. */
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
 * Run `yerd elevate` with no target under OS elevation - applies every step
 * (trust, resolver, ports) in one prompt.
 */
export async function elevateAll(): Promise<void> {
  await call<void>("elevate_all");
}

/**
 * Apply resolver + ports in a single OS-elevation prompt (macOS "Fix all" uses
 * this so the two root steps share one password prompt; trust is in-process).
 */
export async function elevateResolverPorts(): Promise<void> {
  await call<void>("elevate_resolver_ports");
}

/**
 * Revert `elevate` for `target` under the same OS elevation (`yerd unelevate
 * <target>`). On macOS, unelevating the resolver restores the pre-Yerd resolver
 * from its backup (or removes Yerd's file if none). `ports` is reversible on
 * macOS only - callers gate the button accordingly.
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
 * `true` if the CA is *still* trusted afterwards - i.e. a system-wide trust set
 * via the terminal remains, which the GUI can't remove without root.
 */
export async function untrustCa(): Promise<boolean> {
  return call<boolean>("untrust_ca");
}

// ── daemon lifecycle + autostart (host commands, NOT daemon IPC) ────────────

/**
 * Start the daemon. `nudge` (macOS) controls whether a pending Login-Items
 * approval auto-opens System Settings; pass `false` from a flow that enables
 * several login items so they don't each open it (the caller opens it once).
 */
export async function startDaemon(nudge = true): Promise<void> {
  await call<void>("start_daemon", { nudge });
}

export async function stopDaemon(): Promise<void> {
  await call<void>("stop_daemon");
}

/**
 * Gather diagnostics explaining why the daemon isn't up - service-manager
 * status, the daemon's rolling-log tail, binary/socket checks, and host-computed
 * hints. Pass the message a prior `startDaemon` threw (if any) so the
 * never-launched cases (missing binary, translocation, register failure) carry
 * their most actionable signal.
 */
export async function daemonDiagnostics(startError?: string): Promise<DaemonDiagnostics> {
  return call<DaemonDiagnostics>("daemon_diagnostics", { startError });
}

/**
 * macOS: the version of a *newer* registered daemon that this (older) GUI refused
 * to reconfigure/downgrade, or `null` when there's no conflict. Drives the
 * Overview "this Yerd is older than your daemon" banner.
 */
export async function daemonVersionConflict(): Promise<string | null> {
  return call<string | null>("daemon_version_conflict");
}

export async function getAutostart(): Promise<AutostartState> {
  return call<AutostartState>("get_autostart");
}

export async function setAutostartDaemon(on: boolean, nudge = true): Promise<void> {
  await call<void>("set_autostart_daemon", { on, nudge });
}

export async function setAutostartGui(on: boolean, nudge = true): Promise<void> {
  await call<void>("set_autostart_gui", { on, nudge });
}

export async function setAutostartGuiMinimized(on: boolean): Promise<void> {
  await call<void>("set_gui_minimized", { on });
}

export async function getTrayIconVariant(): Promise<TrayIconVariant> {
  return call<TrayIconVariant>("get_tray_icon_variant");
}

export async function setTrayIconVariant(variant: TrayIconVariant): Promise<void> {
  await call<void>("set_tray_icon_variant", { variant });
}

export async function getTitleBarStyle(): Promise<TitleBarStyle> {
  return call<TitleBarStyle>("get_title_bar_style");
}

export async function setTitleBarStyle(style: TitleBarStyle): Promise<void> {
  await call<void>("set_title_bar_style", { style });
}

// ── onboarding / first-run ───────────────────────────────────────────────────

/** First-run decision inputs: has the journey run, and is Yerd already set up? */
export async function setupState(): Promise<SetupState> {
  return call<SetupState>("setup_state");
}

/** Mark the first-run welcome journey complete (persisted host-side). */
export async function markOnboarded(): Promise<void> {
  await call<void>("mark_onboarded");
}

// ── optional: install the bundled `yerd` CLI on PATH (macOS) ────────────────

/** Whether the bundled `yerd` CLI is linked onto PATH (`{data}/bin/yerd`). */
export async function cliPathStatus(): Promise<CliPathStatus> {
  return call<CliPathStatus>("cli_path_status");
}

/** Symlink the bundled `yerd` onto PATH and add `{data}/bin` to the shell rc. */
export async function installCliToPath(): Promise<void> {
  await call<void>("install_cli_to_path");
}

/** Remove the `{data}/bin/yerd` symlink. */
export async function removeCliFromPath(): Promise<void> {
  await call<void>("remove_cli_from_path");
}

/** Open System Settings → Login Items (macOS) to approve the background daemon. */
export async function openLoginItems(): Promise<void> {
  await call<void>("open_login_items");
}

/**
 * macOS: whether the launch-time daemon self-repair thread (`setup_app`) is
 * currently re-registering/kickstarting the daemon. Always false elsewhere -
 * no such thread runs on other OSes.
 */
export async function daemonSelfRepairBusy(): Promise<boolean> {
  return call<boolean>("daemon_self_repair_busy");
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

// ── GUI diagnostic logs (host-only; no daemon IPC) ──────────────────────────

/** Append a line to the per-session GUI host log ({cache}/yerd-gui.log). */
export async function guiLog(level: string, message: string): Promise<void> {
  await call<void>("gui_log", { level, message });
}

/** The GUI session log + a tail of the daemon's own rolling log (About → Logs). */
export async function getGuiLogs(): Promise<GuiLogs> {
  return call<GuiLogs>("get_gui_logs");
}

/** A pretty-printed JSON diagnostics snapshot (paths, service config, ERROR
 * lines from the logs) for the About → Diagnostics button. */
export async function getDiagnostics(): Promise<string> {
  return call<string>("get_diagnostics");
}
