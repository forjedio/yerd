/**
 * TypeScript mirrors of the `yerd-ipc` wire JSON.
 *
 * These are a *contract*, pinned by hand to the Rust source. Each type notes
 * its origin so review catches drift; `tests/wire_stability.rs` and the
 * `Request`/`Response` enums are the source of truth:
 *   - crates/yerd-ipc/src/request.rs   (Request — not consumed here; the GUI
 *     never builds raw Requests, the bridge does — kept for reference)
 *   - crates/yerd-ipc/src/response.rs  (Response, PhpUpdate, ErrorCode)
 *   - crates/yerd-ipc/src/status.rs    (StatusReport and friends)
 *   - crates/yerd-core/src/site.rs     (Site, SiteKind)
 *
 * Wire conventions: enums are internally tagged on `type`, `snake_case`;
 * `PhpVersion` serialises as the string `"8.5"`; `Option<T>` is `T | null`.
 */

// ── core domain ────────────────────────────────────────────────────────────

/** A PHP minor version on the wire is the bare string, e.g. `"8.5"`. */
export type PhpVersion = string;

/** crates/yerd-core/src/site.rs — SiteKind. */
export type SiteKind = "parked" | "linked";

/** crates/yerd-core/src/site.rs — Site (serialised field order is fixed). */
export interface Site {
  name: string;
  document_root: string;
  /** Served web root, relative to document_root (e.g. "public" for Laravel).
   *  Omitted on the wire when empty (= serve the document root itself), so it is
   *  optional here. */
  web_subpath?: string;
  php: PhpVersion;
  secure: boolean;
  kind: SiteKind;
}

// ── status payloads (status.rs) ────────────────────────────────────────────

export interface PortStatus {
  requested: number;
  bound: number;
  fell_back: boolean;
}

export interface CaStatus {
  path: string;
  fingerprint: string;
  /** Tri-state: true / false / null = "probe could not determine" (NOT false). */
  trusted_system: boolean | null;
}

export interface SiteCounts {
  parked: number;
  linked: number;
  secured: number;
}

export type PoolRunState = "running" | "stopped" | "failed";

/** crates/yerd-ipc/src/status.rs — ServiceRunState. */
export type ServiceRunState = "running" | "stopped" | "failed";

/** crates/yerd-ipc/src/status.rs — ServiceStatus (integer-only fields). */
export interface ServiceStatus {
  service: string;
  display_name: string;
  installed_versions: string[];
  selected_version: string | null;
  state: ServiceRunState;
  pid: number | null;
  listen: string | null;
  port: number;
  enabled: boolean;
  supports_databases: boolean;
}

/** crates/yerd-ipc/src/status.rs — ServiceAvailability. */
export interface ServiceAvailability {
  service: string;
  available: string[];
  installed: string[];
}

export interface PhpPoolStatus {
  version: PhpVersion;
  installed_patch: string | null;
  state: PoolRunState;
  pid: number | null;
  listen: string | null;
  rss_bytes: number | null;
  update_available: string | null;
}

/** crates/yerd-ipc/src/status.rs — MailStatus. */
export interface MailStatus {
  enabled: boolean;
  port: number;
  /** Whether the SMTP listener actually bound (enabled && !listening = port busy). */
  listening: boolean;
  count: number;
}

/** crates/yerd-ipc/src/status.rs — MailSummary. */
export interface MailSummary {
  id: string;
  from: string;
  to: string[];
  subject: string;
  /** Unix epoch seconds; 0 when the Date header was absent/unparseable. */
  date_epoch: number;
}

/** crates/yerd-ipc/src/status.rs — MailHeader. */
export interface MailHeader {
  name: string;
  value: string;
}

/** crates/yerd-ipc/src/status.rs — MailDetail. */
export interface MailDetail {
  id: string;
  from: string;
  to: string[];
  subject: string;
  date_epoch: number;
  headers: MailHeader[];
  /** Decoded text/html body (cid: images already rewritten to data: URLs). */
  html_body: string | null;
  text_body: string | null;
}

export interface StatusReport {
  daemon_pid: number;
  uptime_secs: number;
  /** Daemon process RSS in bytes (covers the in-process proxy + DNS). null = unknown. */
  daemon_rss_bytes: number | null;
  tld: string;
  http: PortStatus;
  https: PortStatus;
  dns_addr: string;
  ca: CaStatus;
  /** Tri-state — null means "unknown", never coerce to false. */
  resolver_installed: boolean | null;
  /** macOS: is the pf redirect carrying 80/443 to the rootless ports? true =
   *  privileged ports served via the redirect, false = not, null = unknown / not
   *  applicable (Linux binds directly after setcap). */
  port_redirect: boolean | null;
  /** True when a non-Yerd process is listening on a privileged web port (80/443)
   *  — a foreign squatter. Confirmed via the proxy's Server marker, so it never
   *  misreads Yerd as foreign. false = no conflict, null/undefined = not probed.
   *  Cross-platform (unlike port_redirect). */
  foreign_web_listener?: boolean | null;
  /** macOS: absolute path of the backup Yerd saved when installing the resolver
   *  replaced a pre-existing /etc/resolver/<tld>. Omitted (undefined) when nothing
   *  was replaced or the backup is older than the daemon's reporting window. */
  resolver_backup?: string | null;
  default_php: PhpVersion;
  php: PhpPoolStatus[];
  sites: SiteCounts;
  /** Each entry is load × 100 (hundredths); render via formatLoadAvg. */
  load_avg: [number, number, number] | null;
  /** The daemon's own version (e.g. "2.0.1"). Empty/absent against a daemon
   *  predating version reporting (the Rust field is `#[serde(default)]`); render
   *  "unknown" in that case. */
  daemon_version: string;
  /** Per-service status. Omitted (undefined) by a daemon with no services
   *  (the Rust field is `#[serde(default, skip_serializing_if)]`). */
  services?: ServiceStatus[];
  /** Built-in mail-capture status. Omitted (undefined) by a daemon predating
   *  the feature (the Rust field is `#[serde(default, skip_serializing_if)]`, so
   *  it is never `null` on the wire — mirrors the `services?` convention). */
  mail?: MailStatus;
}

export type Severity = "ok" | "warn" | "fail";

export type DiagnosisCode =
  | "daemon_down"
  | "port_fallback"
  | "foreign_web_listener"
  | "ca_not_trusted"
  | "resolver_not_installed"
  | "no_php_installed"
  | "default_php_not_installed"
  | "fpm_pool_failed"
  | "php_update_available"
  | "no_sites"
  | "resolver_backup_saved"
  | "service_failed"
  | "bin_dir_not_on_path"
  | "all_good";

export interface Diagnosis {
  code: DiagnosisCode;
  severity: Severity;
  title: string;
  detail: string;
  remedy: string | null;
}

export interface FixResult {
  code: DiagnosisCode;
  ok: boolean;
  message: string;
}

export interface FixReport {
  performed: FixResult[];
  manual: Diagnosis[];
}

// ── dumps (dump.rs) ─────────────────────────────────────────────────────────

/** crates/yerd-ipc/src/dump.rs — DumpCategory (the viewer tabs). */
export type DumpCategory =
  | "dump"
  | "query"
  | "job"
  | "view"
  | "request"
  | "log"
  | "cache"
  | "http";

/**
 * crates/yerd-ipc/src/dump.rs — DumpEvent. `payload` is category-specific and
 * opaque to the daemon; the viewer renders it per `category` (see the
 * `yerd-php-ext` architecture doc for the per-category shape).
 */
export interface DumpEvent {
  id: number;
  category: DumpCategory;
  /** Capture time, Unix epoch milliseconds. */
  ts_ms: number;
  /** Originating `.test` site; may be empty. */
  site: string;
  /** Stable per-request id, so the viewer groups rows by request. */
  request_id: string;
  payload: Record<string, unknown>;
}

/** crates/yerd-ipc/src/dump.rs — DumpCounts (current per-category buffer counts). */
export interface DumpCounts {
  dumps: number;
  queries: number;
  jobs: number;
  views: number;
  requests: number;
  logs: number;
  cache: number;
  http: number;
}

/** crates/yerd-ipc/src/dump.rs — DumpExtStatus (per-version extension presence). */
export interface DumpExtStatus {
  version: PhpVersion;
  present: boolean;
}

// ── site creation (create.rs) ───────────────────────────────────────────────
// Unlike most requests, the GUI *does* build this payload (the wizard), so the
// spec types live here and are sent via `createSite`.

/** StarterKit unit variants (externally tagged enum). */
export type StarterKitTag = "none" | "react" | "vue" | "livewire" | "svelte";
/** StarterKit: a unit tag, or a community kit `{ community: "<package>" }`. */
export type StarterKit = StarterKitTag | { community: string };

export type AuthProvider = "laravel" | "work_os";
export type Testing = "pest" | "php_unit";
export type Database = "sqlite" | "mysql" | "mariadb" | "pgsql" | "sqlsrv";
export type JsRuntime = "npm" | "bun" | "skip";

/** crates/yerd-ipc/src/create.rs — LaravelOptions. */
export interface LaravelOptions {
  starter_kit: StarterKit;
  auth: AuthProvider;
  livewire_class_components: boolean;
  teams: boolean;
  testing: Testing;
  database: Database;
  js: JsRuntime;
  git: boolean;
  boost: boolean;
}

/** Framework is internally tagged on `framework`. Only Laravel today. */
export type Framework = { framework: "laravel"; options: LaravelOptions };

/** crates/yerd-ipc/src/create.rs — CreateSiteSpec. */
export interface CreateSiteSpec {
  name: string;
  /** Directory the new project dir is created inside (parked root or any folder). */
  parent_dir: string;
  php: PhpVersion;
  secure: boolean;
  framework: Framework;
}

/** crates/yerd-ipc/src/create.rs — JobState. */
export type JobState = "running" | "succeeded" | "failed" | "cancelled";

// ── response variants (response.rs) ────────────────────────────────────────

export interface PhpUpdate {
  version: PhpVersion;
  installed: string;
  latest: string;
}

export type ErrorCode =
  | "not_found"
  | "already_exists"
  | "invalid_path"
  | "port_in_use"
  | "internal";

/**
 * Response is internally tagged on `type`. The bridge returns the decoded
 * Response directly; helpers below narrow it. A `Response::Error` is converted
 * to a thrown `IpcError` by the client layer, so views never see `type:error`.
 */
export type Response =
  | { type: "pong" }
  | { type: "sites"; sites: Site[] }
  | { type: "ok" }
  | { type: "error"; code: ErrorCode; message: string }
  | { type: "parked"; paths: string[] }
  | {
      type: "info";
      dns_addr: string;
      tld: string;
      ca_path: string;
      ca_fingerprint: string;
      /** Rootless bound ports; `#[serde(default)]` on the Rust side → may be
       *  absent (0) against an older daemon. */
      http_port?: number;
      https_port?: number;
    }
  | {
      type: "php_versions";
      installed: PhpVersion[];
      default: PhpVersion;
      updates?: PhpUpdate[];
      settings?: Record<string, string>;
    }
  | {
      type: "available_php";
      available: PhpVersion[];
      installed: PhpVersion[];
    }
  | { type: "status"; report: StatusReport }
  | { type: "diagnoses"; items: Diagnosis[] }
  | { type: "doctor_fix"; report: FixReport }
  | { type: "services"; services: ServiceStatus[] }
  | { type: "available_services"; services: ServiceAvailability[] }
  | { type: "service_logs"; lines: string[] }
  | { type: "databases"; databases: DatabaseSummary[] }
  | {
      type: "dumps";
      events: DumpEvent[];
      removed_ids: number[];
      counts: DumpCounts;
      latest_id: number;
      /** Smallest id still buffered; drop any held id below this. */
      min_live_id: number;
    }
  | {
      type: "dumps_status";
      enabled: boolean;
      port: number;
      running: boolean;
      /** Whether logs persist across requests (off = clear on each new request). */
      persist: boolean;
      extensions: DumpExtStatus[];
      counts: DumpCounts;
      /** Resolved per-feature flags (every key present). */
      features: Record<string, boolean>;
    }
  | { type: "mails"; mails: MailSummary[] }
  | { type: "mail"; mail: MailDetail }
  | { type: "tools"; tools: ToolStatus[] }
  | { type: "job_started"; job_id: string }
  | {
      type: "job_progress";
      state: JobState;
      phase: string;
      /** Log lines newer than the polled cursor, oldest first. */
      log: string[];
      next_cursor: number;
      error: string | null;
    }
  | {
      type: "update_status";
      /** The running Yerd version. */
      current: string;
      /** Highest stable version available, or null if none/unknown. */
      latest_stable: string | null;
      /** Highest edge (pre-release-inclusive) version available, or null. */
      latest_edge: string | null;
      /** The channel this check resolved against. */
      channel: UpdateChannel;
      /** Whether a newer version is available on `channel`. */
      available: boolean;
      /** The version `channel` would update to, or null when up to date. */
      target: string | null;
      /** Running a pre-release ahead of stable (stable would be a downgrade). */
      ahead_of_stable: boolean;
      /** Whether these figures are from a live fetch or a cached fallback. */
      source: "live" | "cached";
    };

/** Self-update release channel (mirrors `yerd_ipc::Channel`). */
export type UpdateChannel = "stable" | "edge";

/** crates/yerd-ipc/src/status.rs — ToolStatus. */
export interface ToolStatus {
  id: string;
  display_name: string;
  installed: boolean;
  version: string | null;
  binaries: string[];
}

/** One user database in a SQL service (mirrors the daemon's `DatabaseSummary`). */
export interface DatabaseSummary {
  name: string;
}

// Narrowed aliases for the variants the views actually read.
export type InfoResponse = Extract<Response, { type: "info" }>;
export type SitesResponse = Extract<Response, { type: "sites" }>;
export type ParkedResponse = Extract<Response, { type: "parked" }>;
export type PhpVersionsResponse = Extract<Response, { type: "php_versions" }>;
export type UpdateStatusResponse = Extract<Response, { type: "update_status" }>;
export type AvailablePhpResponse = Extract<Response, { type: "available_php" }>;
export type StatusResponse = Extract<Response, { type: "status" }>;
export type DiagnosesResponse = Extract<Response, { type: "diagnoses" }>;
export type DoctorFixResponse = Extract<Response, { type: "doctor_fix" }>;
export type ServicesResponse = Extract<Response, { type: "services" }>;
export type AvailableServicesResponse = Extract<Response, { type: "available_services" }>;
export type ServiceLogsResponse = Extract<Response, { type: "service_logs" }>;
export type DumpsResponse = Extract<Response, { type: "dumps" }>;
export type DumpsStatusResponse = Extract<Response, { type: "dumps_status" }>;
export type MailsResponse = Extract<Response, { type: "mails" }>;
export type MailResponse = Extract<Response, { type: "mail" }>;
export type JobStartedResponse = Extract<Response, { type: "job_started" }>;
export type JobProgressResponse = Extract<Response, { type: "job_progress" }>;

/** Privilege targets for the OS-elevated `yerd elevate` host command. */
export type ElevateTarget = "trust" | "resolver" | "ports";

/**
 * Login-autostart state for the General tab (host command `get_autostart`).
 * `daemonSupported` is false when no per-user service manager exists (e.g. a
 * Linux box without systemd `--user`), in which case the daemon toggle is
 * disabled in the UI.
 */
export interface AutostartState {
  daemon: boolean;
  daemonSupported: boolean;
  gui: boolean;
  guiMinimized: boolean;
  /** macOS only: registered but awaiting approval in System Settings → Login Items. */
  daemonPendingApproval: boolean;
}

/**
 * Whether the bundled `yerd` CLI is symlinked onto PATH (macOS host command
 * `cli_path_status`). On Linux the `.deb` already puts `yerd` on PATH, so the
 * control is hidden there.
 */
export interface CliPathStatus {
  installed: boolean;
  target: string;
}
