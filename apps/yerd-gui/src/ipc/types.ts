/**
 * TypeScript mirrors of the `yerd-ipc` wire JSON.
 *
 * These are a *contract*, pinned by hand to the Rust source. Each type notes
 * its origin so review catches drift; `tests/wire_stability.rs` and the
 * `Request`/`Response` enums are the source of truth:
 *   - crates/yerd-ipc/src/request.rs   (Request - not consumed here; the GUI
 *     never builds raw Requests, the bridge does - kept for reference)
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

/** crates/yerd-core/src/site.rs - SiteKind. */
export type SiteKind = "parked" | "linked";

/** crates/yerd-core/src/site.rs - Site (serialised field order is fixed). */
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
  /** WordPress one-click admin login toggle. Omitted on the wire when false. */
  wp_auto_login?: boolean;
  /** WordPress login/username to sign in as, or absent for "earliest admin". */
  wp_auto_login_user?: string;
  /** Stored front-controller override: absent = auto (derive from detection),
   *  otherwise the explicit user choice. Read `SiteEntry.uses_front_controller`
   *  for the effective value to display. */
  front_controller?: boolean;
}

/**
 * One entry in `Response::Sites` - `Site` plus a WordPress-detection flag
 * served from the daemon's in-memory cache (`SiteEntry` in response.rs,
 * `#[serde(flatten)]`, so this is a flat object on the wire - no nested
 * `site` key). Omitted on the wire when absent.
 */
export interface SiteEntry extends Site {
  is_wordpress?: boolean;
  /** Primary (canonical) domain FQDN, present only when it differs from the
   *  default `{name}.{tld}` apex (omitted otherwise). */
  primary_domain?: string;
  /** Full effective routable domain set as FQDNs, in router order (apex-first,
   *  so the primary is not necessarily first - match `primary_domain` by value).
   *  Present only for a customised site (omitted for a default apex-only site). */
  domains?: string[];
  /** Another site's name that claims this site's apex label, when shadowed. */
  apex_shadowed_by?: string;
  /** Effective front-controller mode the daemon resolved (stored override or
   *  detected default). `true` funnels through `index.php`; `false` executes
   *  named `.php` directly. Always present; absent only from an older daemon. */
  uses_front_controller?: boolean;
  /** Whether an `artisan` marker was found at the site's project root, i.e. it
   *  is a Laravel app (eligible to link a Reverb instance). */
  is_laravel?: boolean;
}

/** crates/yerd-ipc/src/response.rs - WordPressAdminUser, for the auto-login
 *  user picker (`Response::WordpressAdminUsers`). */
export interface WordPressAdminUser {
  login: string;
  display_name: string;
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
  /**
   * Whether the bundled PHP trusts the Yerd CA (managed `cacert.pem` present +
   * contains the CA). Optional/`null` when the feature is off or undeterminable.
   */
  php_trusts_ca?: boolean | null;
}

export interface SiteCounts {
  parked: number;
  linked: number;
  secured: number;
}

export type PoolRunState = "running" | "stopped" | "failed";

/** crates/yerd-ipc/src/status.rs - ServiceRunState. */
export type ServiceRunState = "running" | "stopped" | "failed";

/** crates/yerd-ipc/src/status.rs - ServiceStatus (integer-only fields). */
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
  /** Whether the engine accepts free-form configuration overrides. Omitted by
   *  the daemon when false, and absent entirely from an older daemon. */
  supports_overrides?: boolean;
  /** Service type id (`"redis"`, `"reverb"`), distinct from `service` (the
   *  instance wire id, e.g. `"reverb:blog"`). Falls back to `service` when
   *  omitted by an older daemon. */
  type_id?: string;
  /** Linked site for a per-site instance; absent for single-instance engines. */
  site?: string | null;
  /** Last failure message when `state` is `"failed"`. */
  error?: string | null;
}

/** crates/yerd-ipc/src/status.rs - ServiceAvailability. */
export interface ServiceAvailability {
  service: string;
  available: string[];
  installed: string[];
}

/** crates/yerd-ipc/src/status.rs - AddableServiceType (the "Add Service" dialog
 *  catalog). */
export interface AddableServiceType {
  type_id: string;
  display_name: string;
  multiplicity: "single" | "per_site";
  requires_site: boolean;
  requires_version: boolean;
  already_installed: boolean;
  available_versions: string[];
  default_port: number;
  suggested_port: number;
}

/** crates/yerd-ipc/src/status.rs - WordPressVersionInfo. */
export interface WordPressVersionInfo {
  branch: string;
  latest: string;
  min_php: PhpVersion;
  max_php: PhpVersion;
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

/** crates/yerd-ipc/src/status.rs - MailStatus. */
export interface MailStatus {
  enabled: boolean;
  port: number;
  /** Whether the SMTP listener actually bound (enabled && !listening = port busy). */
  listening: boolean;
  count: number;
  /** Captured emails not yet marked read (absent against an older daemon). */
  unread?: number;
}

/** crates/yerd-ipc/src/status.rs - MailSummary. */
export interface MailSummary {
  id: string;
  from: string;
  to: string[];
  subject: string;
  /** Unix epoch seconds; 0 when the Date header was absent/unparseable. */
  date_epoch: number;
  /** Whether the email has been marked read (absent against an older daemon). */
  read?: boolean;
}

/** crates/yerd-ipc/src/status.rs - MailHeader. */
export interface MailHeader {
  name: string;
  value: string;
}

/** crates/yerd-ipc/src/status.rs - MailAttachment. */
export interface MailAttachment {
  filename: string;
  content_type: string;
  /** Decoded byte count (before base64 encoding). */
  size: number;
  /** Standard (padded) base64-encoded attachment bytes. */
  data: string;
}

/** crates/yerd-ipc/src/status.rs - MailDetail. */
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
  /** Non-inline attachments. Omitted on the wire (and therefore undefined here)
   *  when the message has none, or when talking to an older daemon. */
  attachments?: MailAttachment[];
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
  /** Tri-state - null means "unknown", never coerce to false. */
  resolver_installed: boolean | null;
  /** macOS: is the port forwarder carrying 80/443 to the rootless ports? true =
   *  privileged ports served via the redirect, false = not, null/undefined =
   *  unknown / not applicable (Linux binds directly after setcap). Omitted on the
   *  wire when not set (the Rust field is `#[serde(default, skip_serializing_if)]`,
   *  so it can be absent against a Linux/older daemon). */
  port_redirect?: boolean | null;
  /** True when a non-Yerd process is listening on a privileged web port (80/443)
   *  - a foreign squatter. Confirmed via the proxy's Server marker, so it never
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
   *  it is never `null` on the wire - mirrors the `services?` convention). */
  mail?: MailStatus;
  /** Set when the daemon could bind neither the desired nor the fallback web
   *  ports: it runs degraded (no HTTP/HTTPS proxy). Carries the fallback ports
   *  it failed on. Omitted (undefined) on a healthy daemon. */
  web_unbound?: { http: number; https: number } | null;
  /** Set when the daemon couldn't bind its DNS responder port: it runs degraded
   *  (`*.test` names won't resolve through Yerd). Carries the configured DNS port
   *  it failed on. Omitted (undefined) on a healthy daemon. */
  dns_unbound?: number | null;
  /** A per-process id that changes on every (re)start. Clients use a *change* in
   *  it to confirm a restart completed (the re-exec preserves the pid). Omitted
   *  by a daemon predating the field. */
  boot_id?: number | null;
  /** Number of sites currently shared publicly (quick tunnels + named-tunnel
   *  exposed sites). Omitted/`0` when nothing is shared. */
  shared_sites?: number;
  /** Whether the proxy's symlink-escape protection is on. `true` = protection
   *  active (block symlinks resolving outside a site's document root); `false` =
   *  the user has opted out. Defaults to `true` (protected) when omitted by a
   *  daemon predating the field. */
  symlink_protection?: boolean;
  /** Sites whose apex label is shadowed by another site's explicit domain.
   *  Omitted/empty on a healthy config. */
  shadows?: DomainShadow[];
  /** Whether `yerd mcp` serves Yerd's tools to local AI agents. Defaults to
   *  `false` (opt-in) when omitted by a daemon predating the field. */
  mcp_enabled?: boolean;
  /** macOS: destination ports the installed loopback port forwarder (or a
   *  legacy pf anchor) carries 80/443 to. Compare with `http.bound`/`https.bound` to detect a
   *  stale redirect that black-holes on-host access. Omitted when not installed,
   *  unreadable, or not macOS. */
  port_redirect_targets?: { http: number; https: number } | null;
  /** macOS: same as `port_redirect_targets` for the LAN (`dev.yerd.lan`) anchor.
   *  A stale target here black-holes other devices while LAN mode is on. */
  lan_redirect_targets?: { http: number; https: number } | null;
  /** macOS: whether a pf redirect from an older Yerd is still installed, so
   *  80/443 still go through pf instead of the launchd port forwarder. Omitted
   *  when not macOS or undeterminable. */
  legacy_pf_redirect?: boolean | null;
  /** Installed Node releases with a newer patch (cached check). Absent when 0. */
  node_updates?: number;
}

/** One apex-shadow relationship: `site`'s apex is claimed by `shadowed_by`. */
export interface DomainShadow {
  site: string;
  shadowed_by: string;
}

export type Severity = "ok" | "warn" | "fail";

export type DiagnosisCode =
  | "daemon_down"
  | "port_fallback"
  | "web_ports_unbound"
  | "foreign_web_listener"
  | "dns_port_unbound"
  | "ca_not_trusted"
  | "ca_not_trusted_by_browsers"
  | "resolver_not_installed"
  | "no_php_installed"
  | "default_php_not_installed"
  | "fpm_pool_failed"
  | "php_update_available"
  | "no_sites"
  | "resolver_backup_saved"
  | "service_failed"
  | "service_override_invalid"
  | "bin_dir_not_on_path"
  | "php_ca_not_trusted"
  | "symlink_protection_disabled"
  | "domain_shadowed"
  | "port_redirect_stale"
  | "lan_redirect_stale"
  | "legacy_port_redirect"
  | "outbound_proxy_unreachable"
  | "outbound_proxy_pac_unsupported"
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

/** crates/yerd-ipc/src/dump.rs - DumpCategory (the viewer tabs). */
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
 * crates/yerd-ipc/src/dump.rs - DumpEvent. `payload` is category-specific and
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

/** crates/yerd-ipc/src/dump.rs - DumpCounts (current per-category buffer counts). */
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

/** crates/yerd-ipc/src/dump.rs - DumpExtStatus (per-version extension presence). */
export interface DumpExtStatus {
  version: PhpVersion;
  present: boolean;
  /** True for legacy (< 8.2) versions, which never capture dumps. */
  legacy?: boolean;
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
export type JsRuntime = "npm" | "bun" | "pnpm" | "skip";

/** crates/yerd-ipc/src/create.rs - LaravelOptions. */
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
  /** Installed Node release to build with and save as the site's preference. Omitted for the default. */
  node?: string;
}

export type WordPressDbEngine = "mysql" | "mariadb";

/** crates/yerd-ipc/src/create.rs - WordPressDatabase. */
export interface WordPressDatabase {
  engine: WordPressDbEngine;
  name: string;
}

/** crates/yerd-ipc/src/create.rs - WordPressOptions. */
export interface WordPressOptions {
  /** `null` installs the latest stable release. */
  core_version: string | null;
  locale: string;
  admin_user: string;
  admin_email: string;
  admin_password: string;
  site_title: string;
  table_prefix: string;
  database: WordPressDatabase;
}

/**
 * Framework is internally tagged on `framework`. The Rust variant is spelled
 * `Wordpress` (one capital) so `rename_all = "snake_case"` produces the wire
 * tag `"wordpress"` rather than `"word_press"` - see create.rs's doc comment.
 */
export type Framework =
  | { framework: "laravel"; options: LaravelOptions }
  | { framework: "wordpress"; options: WordPressOptions }
  | { framework: ComposerFramework };

/** Frameworks scaffolded with `composer create-project`, which take no options. */
export type ComposerFramework = "codeigniter" | "cakephp" | "slim";

/** crates/yerd-ipc/src/create.rs - CreateSiteSpec. */
export interface CreateSiteSpec {
  name: string;
  /** Directory the new project dir is created inside (parked root or any folder). */
  parent_dir: string;
  php: PhpVersion;
  secure: boolean;
  framework: Framework;
}

/** crates/yerd-ipc/src/create.rs - JobState. */
export type JobState = "running" | "succeeded" | "failed" | "cancelled";

// ── response variants (response.rs) ────────────────────────────────────────

export interface PhpUpdate {
  version: PhpVersion;
  installed: string;
  latest: string;
}

/** One registered custom PHP extension (see the `php_extensions` response). */
export interface PhpExtInfo {
  name: string;
  path: string;
  zend: boolean;
  /** Whether the `.so` currently exists on disk (false = broken registration). */
  present: boolean;
}

/** One Xdebug mode (a member of the `xdebug.mode` list). `off` is not a mode:
 *  turning Xdebug off is the toggle's job. */
export type XdebugMode =
  | "debug"
  | "develop"
  | "coverage"
  | "profile"
  | "trace"
  | "gcstats";

/** When Xdebug starts a session: on every request, or only with a trigger. */
export type XdebugStart = "always" | "trigger";

/** One PHP version's Xdebug settings (see the `php_versions` response). */
export interface XdebugSettings {
  enabled: boolean;
  modes: XdebugMode[];
  start: XdebugStart;
}

export type ErrorCode =
  | "not_found"
  | "already_exists"
  | "invalid_path"
  | "port_in_use"
  | "extension_load_failed"
  | "legacy_restricted"
  | "xdebug_unavailable"
  | "invalid_setting"
  | "internal";

/** One whole-host reverse proxy (`{name}.{tld}` → `target`). Reply element of a
 *  `"proxies"` response. */
export interface ProxyEntry {
  name: string;
  target: string;
  secure: boolean;
  /** Primary (canonical) domain FQDN, present only when it differs from the
   *  default `{name}.{tld}` apex (omitted otherwise). */
  primary_domain?: string;
  /** Full effective routable domain set as FQDNs, in router order (apex-first,
   *  so the primary is not necessarily first - match `primary_domain` by value).
   *  Present only for a customised proxy (omitted for a default apex-only one). */
  domains?: string[];
}

/** One per-site path-prefix reverse-proxy rule (`{site}{prefix}` → `target`).
 *  Reply element of a `"proxies"` response. */
export interface ProxyRuleEntry {
  site: string;
  prefix: string;
  target: string;
}

/** One per-site path-prefix routing rule (`{site}{prefix}` → a file at `target`,
 *  relative to the site's web root). Reply element of a `"routes"` response.
 *  Unlike {@link ProxyRuleEntry}, whose target is an upstream URL, this resolves
 *  inside the site itself. */
export interface RouteRuleEntry {
  site: string;
  prefix: string;
  target: string;
}

/** One per-site custom response header, added to every response the site
 *  serves. Reply element of a `"response_headers"` response. */
export interface ResponseHeaderEntry {
  site: string;
  name: string;
  value: string;
}

/**
 * Response is internally tagged on `type`. The bridge returns the decoded
 * Response directly; helpers below narrow it. A `Response::Error` is converted
 * to a thrown `IpcError` by the client layer, so views never see `type:error`.
 */
export type Response =
  | { type: "pong" }
  | { type: "sites"; sites: SiteEntry[] }
  | { type: "ok" }
  | { type: "error"; code: ErrorCode; message: string }
  | { type: "parked"; paths: string[] }
  | { type: "proxies"; proxies: ProxyEntry[]; rules: ProxyRuleEntry[] }
  | { type: "routes"; rules: RouteRuleEntry[] }
  | { type: "response_headers"; headers: ResponseHeaderEntry[] }
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
      /** Configured rootless fallback ports (what Settings edits). Distinct from
       *  `http_port`/`https_port`, which are the *bound* ports. `#[serde(default)]`
       *  → may be absent (0) against an older daemon. */
      fallback_http?: number;
      fallback_https?: number;
      /** Configured DNS responder port (what Settings edits). Distinct from
       *  `dns_addr`, which is the *bound* address. `#[serde(default)]` → may be
       *  absent (0) against an older daemon. */
      dns_port?: number;
    }
  | {
      type: "php_versions";
      installed: PhpVersion[];
      default: PhpVersion;
      updates?: PhpUpdate[];
      settings?: Record<string, string>;
      /** Sparse per-version overrides of `settings`, keyed by version string
       *  (e.g. `"8.3"`). Absent when no overrides are set / older daemon. */
      version_settings?: Record<PhpVersion, Record<string, string>>;
      /** Free-form per-version ini directives keyed by version string. Absent
       *  when none are set / older daemon. */
      directives?: Record<PhpVersion, Record<string, string>>;
      /** Per-version FPM pool overrides (currently only `max_children`) keyed
       *  by version string. A version that is absent runs the built-in
       *  default of 16. Absent when none are set / older daemon. */
      pool?: Record<PhpVersion, Record<string, string>>;
      /** Installed versions whose PHP build includes the FFI extension. Absent
       *  when none do / older daemon. */
      ffi_versions?: PhpVersion[];
      /** Whether libvips (needed by php-vips) was found on the host. Absent
       *  when the daemon doesn't check on this OS / older daemon. */
      libvips_found?: boolean | null;
      /** Per-version Xdebug settings keyed by version string. Only versions
       *  whose settings differ from the default (off, debug, always) appear.
       *  Absent when none are configured / older daemon. */
      xdebug?: Record<PhpVersion, XdebugSettings>;
      /** Installed versions a bundled Xdebug is published for on this host.
       *  Absent when none / older daemon. */
      xdebug_versions?: PhpVersion[];
    }
  | {
      type: "available_php";
      available: PhpVersion[];
      installed: PhpVersion[];
      /** Installable legacy (< 8.2) minors from php-legacy.json; omitted when none. */
      legacy?: PhpVersion[];
    }
  | {
      type: "php_extensions";
      /** Registered extensions keyed by version string (e.g. `"8.5"`). */
      by_version: Record<PhpVersion, PhpExtInfo[]>;
    }
  | { type: "status"; report: StatusReport }
  | { type: "diagnoses"; items: Diagnosis[] }
  | { type: "doctor_fix"; report: FixReport }
  | { type: "services"; services: ServiceStatus[] }
  | { type: "available_services"; services: ServiceAvailability[] }
  | { type: "addable_services"; types: AddableServiceType[] }
  | { type: "service_instance_id"; id: string }
  | { type: "wordpress_versions"; versions: WordPressVersionInfo[] }
  | { type: "wordpress_login_token"; token: string }
  | { type: "wordpress_admin_users"; users: WordPressAdminUser[] }
  | { type: "service_logs"; lines: string[] }
  | { type: "service_overrides"; overrides: Record<string, string> }
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
  | { type: "node_versions"; installed: NodeRelease[] }
  | { type: "available_node"; majors: MajorLine[] }
  | {
      type: "node_site";
      /** The saved site preference (a selector), when one is set. */
      preference?: string;
      /** The nearest `.nvmrc` / `.node-version` inside the site, when found. */
      project_file?: NodeProjectFile;
      /** The exact installed release the site resolves to. */
      effective?: string;
      /** Where `effective` came from. */
      source?: NodeSource;
      /** Why no release could be resolved (missing release, unreadable file, unknown site). */
      error?: string;
      /** The chosen selector that matches no installed release, so it can be installed. */
      missing?: string;
    }
  | { type: "network_settings"; settings: NetworkSettings }
  | { type: "network_test"; result: NetworkTestResult }
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
      /** Unix epoch (seconds) when this result was obtained, for a "last
       *  checked …" display. Absent/undefined when never checked. */
      checked_at_epoch?: number;
    }
  | {
      type: "tunnels";
      tunnels: TunnelInfo[];
      cloudflared: CloudflaredStatus;
    }
  | {
      type: "named_tunnels";
      tunnels: NamedTunnelMeta[];
      sites: SiteHostname[];
      /** Authorized Cloudflare zone (domain), when resolvable. */
      zone?: string;
    }
  | ({ type: "groups" } & GroupsState);

/** crates/yerd-config/src/schema.rs - GroupsSection (mirrored on the wire by the
 *  daemon's `Response::Groups`). Groups are a GUI organisational overlay for the
 *  Sites view; they do not affect routing. */
export interface GroupsState {
  /** Group display names, in display order (the index is the ordering). */
  order: string[];
  /** Per-site membership: site name → group name. A site absent from the map is
   *  ungrouped (the synthetic "Unallocated" bucket). */
  members: Record<string, string>;
}

/** Self-update release channel (mirrors `yerd_ipc::Channel`). */
export type UpdateChannel = "stable" | "edge";

/** crates/yerd-ipc/src/status.rs - ToolStatus. */
export interface ToolStatus {
  id: string;
  display_name: string;
  installed: boolean;
  version: string | null;
  binaries: string[];
  /**
   * Not Yerd-managed but available on the user's PATH (Homebrew / fnm / global
   * install). Skipped on the wire when false (hence optional). Mutually
   * exclusive with `installed`.
   */
  external?: boolean;
  /**
   * Where the external tool was found on the user's PATH (e.g.
   * `/opt/homebrew/bin/node`), when `external` is true. Not guaranteed to be
   * absolute. Skipped on the wire (key absent, not null) when there's nothing
   * to report.
   */
  external_path?: string;
}

// ── node versions (crates/yerd-ipc/src/node.rs, crates/yerd-core/src/node.rs) ──

/** crates/yerd-core/src/node.rs - NodeSource. Where a site's Node version comes from. */
export type NodeSource = "nvmrc" | "node-version" | "site" | "default";

/** crates/yerd-ipc/src/node.rs - NodePreferenceRef. A saved per-project preference. */
export interface NodePreferenceRef {
  /** Canonical project root the preference is keyed by. */
  root: string;
  /** The registered site with that root, when there is one. */
  site?: string;
}

/** crates/yerd-ipc/src/node.rs - NodeRelease. One installed Node release. */
export interface NodeRelease {
  /** Exact version, e.g. `22.12.0`. */
  version: string;
  lts: boolean;
  /** LTS codename (e.g. `Jod`), when known. */
  codename?: string;
  /** This release is what the effective global default resolves to. */
  default: boolean;
  /** Uninstalling is refused: others remain and the global default would no longer resolve. */
  uninstall_blocked: boolean;
  /** Saved preferences an uninstall of this release would clear. Omitted when empty. */
  clears_preferences?: NodePreferenceRef[];
  /** Newer patch of this release's major from the daemon's last check; only on the highest installed patch. */
  update?: string;
}

/** crates/yerd-core/src/node.rs - MajorLine. One major from the nodejs.org index. */
export interface MajorLine {
  major: number;
  /** Newest installable patch of this major. */
  latest: string;
  codename?: string;
  /** End-of-life status; absent when the release schedule is unavailable. */
  eol?: boolean;
  /** Highest installed patch of this major, when any is installed. */
  installed_latest?: string;
  update_available: boolean;
}

/** crates/yerd-ipc/src/node.rs - NodeProjectFile. */
export interface NodeProjectFile {
  /** Only ever `nvmrc` or `node-version`. */
  kind: NodeSource;
  selector: string;
}

/** One user database in a SQL service (mirrors the daemon's `DatabaseSummary`). */
export interface DatabaseSummary {
  name: string;
}

// ── tunnels (status.rs - Cloudflare Tunnel integration) ──────────────────────

/** crates/yerd-ipc/src/status.rs - TunnelKind. */
export type TunnelKind = "quick" | "named";

/** crates/yerd-ipc/src/status.rs - TunnelRunState. */
export type TunnelRunState = "running" | "failed" | "connecting" | "reconnecting";

/** crates/yerd-ipc/src/status.rs - TunnelInfo. */
export interface TunnelInfo {
  site: string;
  kind: TunnelKind;
  state: TunnelRunState;
  /** Public URL of a Quick tunnel once captured; absent otherwise. */
  url?: string;
  /** Configured public hostname of a Named tunnel; absent otherwise. */
  hostname?: string;
  /** Most recent error reported for the tunnel; absent when none. */
  last_error?: string;
  /** Whether the shared site has a share password; absent means false. */
  protected?: boolean;
  /** Preflight warnings computed when the share started; absent when none. */
  warnings?: string[];
}

/** crates/yerd-ipc/src/status.rs - CloudflaredSource. */
export type CloudflaredSource = "managed" | "system";

/** crates/yerd-ipc/src/status.rs - CloudflaredStatus. */
export interface CloudflaredStatus {
  installed: boolean;
  /** Installed cloudflared version when known; absent otherwise. */
  version?: string;
  /** Where the installed binary came from; absent when not installed. */
  source?: CloudflaredSource;
  /** Whether a Cloudflare account is logged in (Phase 2). */
  logged_in: boolean;
}

/** crates/yerd-ipc/src/status.rs - NamedTunnelMeta. */
export interface NamedTunnelMeta {
  name: string;
  uuid: string;
}

/** crates/yerd-ipc/src/status.rs - SiteHostname (a site enabled in the named tunnel). */
export interface SiteHostname {
  site: string;
  hostname: string;
  /** Whether the site has a share password; absent means false. */
  protected?: boolean;
}

// Narrowed aliases for the variants the views actually read.
export type InfoResponse = Extract<Response, { type: "info" }>;
export type SitesResponse = Extract<Response, { type: "sites" }>;
export type ParkedResponse = Extract<Response, { type: "parked" }>;
export type ProxiesResponse = Extract<Response, { type: "proxies" }>;
export type RoutesResponse = Extract<Response, { type: "routes" }>;
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
export type NodeVersionsResponse = Extract<Response, { type: "node_versions" }>;
export type AvailableNodeResponse = Extract<Response, { type: "available_node" }>;
export type NodeSiteResponse = Extract<Response, { type: "node_site" }>;

/** How the daemon picks its outbound proxy. */
export type OutboundProxyMode = "system" | "manual" | "off";

/** Where the resolved outbound proxy came from. */
export type ProxySource =
  | "manual"
  | "daemon_env"
  | "macos_system_settings"
  | "gnome_settings"
  | "login_shell"
  | "none";

/** The proxy the daemon is actually using. URLs are redacted (`***`). */
export interface EffectiveProxy {
  mode: OutboundProxyMode;
  source: ProxySource;
  http_proxy?: string;
  https_proxy?: string;
  /** Effective bypass list, loopback and the site TLD included. */
  bypass: string[];
}

/** Saved outbound-proxy settings plus what they resolve to. URLs are redacted. */
export interface NetworkSettings {
  mode: OutboundProxyMode;
  http_proxy?: string;
  https_proxy?: string;
  /** User bypass entries. */
  no_proxy: string[];
  effective?: EffectiveProxy;
}

/** Result of a connection test through the resolved proxy. */
export interface NetworkTestResult {
  ok: boolean;
  url: string;
  status?: number;
  error?: string;
  /** Redacted proxy the request went through; absent for a direct request. */
  via?: string;
  source: ProxySource;
}
export type JobStartedResponse = Extract<Response, { type: "job_started" }>;
export type JobProgressResponse = Extract<Response, { type: "job_progress" }>;
export type TunnelsResponse = Extract<Response, { type: "tunnels" }>;
export type NamedTunnelsResponse = Extract<Response, { type: "named_tunnels" }>;
export type GroupsResponse = Extract<Response, { type: "groups" }>;

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
  /** macOS only: the GUI login item is registered but awaiting approval in Login Items. */
  guiPendingApproval: boolean;
}

/** Host-detected IDE option returned by `get_installed_ides`. */
export interface IdeOption {
  id: string;
  label: string;
}

/**
 * Tray icon appearance (host commands `get_tray_icon_variant` /
 * `set_tray_icon_variant`). `"auto"` (default) keeps the per-OS default: macOS
 * auto-tints a monochrome template, other OSes show the full color app icon.
 */
export type TrayIconVariant = "auto" | "light-y" | "dark-y" | "full";

/**
 * Title bar control style (host commands `get_title_bar_style` /
 * `set_title_bar_style`). `"auto"` (default) matches the host OS convention,
 * resolved client-side from `host_platform`. Drawn entirely by the frontend -
 * the Rust side only persists the preference.
 */
export type TitleBarStyle = "auto" | "macos" | "linux" | "linux-reversed" | "windows";

/**
 * Why the daemon isn't up (host command `daemon_diagnostics`). Gathered when a
 * start attempt fails to connect - covers both the ran-and-crashed case
 * (`logTail`) and the never-launched cases (`startError`, `translocated`,
 * `yerddPath === null`, `pendingApproval`). `hints` are ready-to-show
 * plain-English cause+fix lines computed host-side.
 */
export interface DaemonDiagnostics {
  startError: string | null;
  hints: string[];
  yerddPath: string | null;
  translocated: boolean;
  socketPath: string;
  socketResponding: boolean;
  lastConnectError: string | null;
  serviceManager: string;
  serviceStatus: string | null;
  pendingApproval: boolean;
  logPath: string | null;
  logTail: string[];
  spawnLogTail: string[];
  /** GUI daemon-registration self-repair trail ({cache}/yerd-gui-repair.log) -
   *  macOS upgrade re-registration attempts + outcomes (incl. technical errors). */
  repairLogTail: string[];
}

/**
 * Whether the bundled `yerd` CLI is symlinked onto PATH (macOS host command
 * `cli_path_status`). On Linux the package (`.deb`/`.pkg.tar.zst`) already puts
 * `yerd` on PATH, so the control is hidden there.
 */
export interface CliPathStatus {
  installed: boolean;
  target: string;
}

/**
 * First-run decision inputs (host command `setup_state`). The welcome journey
 * shows only when `!onboarded && !isSetUp` and the daemon is unreachable.
 */
export interface SetupState {
  onboarded: boolean;
  isSetUp: boolean;
}

/**
 * The GUI diagnostic logs surfaced in About → GUI Logs (host command
 * `get_gui_logs`): the per-session GUI host log plus a tail of the daemon's own
 * rolling log. Paths are shown so the dialog can say where each lives.
 */
export interface GuiLogs {
  guiPath: string | null;
  guiLog: string[];
  daemonPath: string | null;
  daemonLog: string[];
}
