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

export interface PhpPoolStatus {
  version: PhpVersion;
  installed_patch: string | null;
  state: PoolRunState;
  pid: number | null;
  listen: string | null;
  rss_bytes: number | null;
  update_available: string | null;
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
  default_php: PhpVersion;
  php: PhpPoolStatus[];
  sites: SiteCounts;
  /** Each entry is load × 100 (hundredths); render via formatLoadAvg. */
  load_avg: [number, number, number] | null;
  /** The daemon's own version (e.g. "2.0.1"). Empty/absent against a daemon
   *  predating version reporting (the Rust field is `#[serde(default)]`); render
   *  "unknown" in that case. */
  daemon_version: string;
}

export type Severity = "ok" | "warn" | "fail";

export type DiagnosisCode =
  | "daemon_down"
  | "port_fallback"
  | "ca_not_trusted"
  | "resolver_not_installed"
  | "no_php_installed"
  | "default_php_not_installed"
  | "fpm_pool_failed"
  | "php_update_available"
  | "no_sites"
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
  | { type: "doctor_fix"; report: FixReport };

// Narrowed aliases for the variants the views actually read.
export type InfoResponse = Extract<Response, { type: "info" }>;
export type SitesResponse = Extract<Response, { type: "sites" }>;
export type ParkedResponse = Extract<Response, { type: "parked" }>;
export type PhpVersionsResponse = Extract<Response, { type: "php_versions" }>;
export type AvailablePhpResponse = Extract<Response, { type: "available_php" }>;
export type StatusResponse = Extract<Response, { type: "status" }>;
export type DiagnosesResponse = Extract<Response, { type: "diagnoses" }>;
export type DoctorFixResponse = Extract<Response, { type: "doctor_fix" }>;

/** Privilege targets for the OS-elevated `yerd elevate` host command. */
export type ElevateTarget = "trust" | "resolver" | "ports";
