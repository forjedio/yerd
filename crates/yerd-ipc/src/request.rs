//! Client → daemon request envelope.
//!
//! Internally tagged on `type`, `snake_case`. Treat this enum as a
//! published contract - add variants and fields additively, never
//! rename, and let `tests/wire_stability.rs` pin the byte-exact wire
//! shape.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::PhpVersion;

// IMPORTANT: per-field serde renames are forbidden in this crate. Add
// new variants/fields additively; let rename_all handle casing. See
// README and the verification script's grep gate.
/// A request sent from a client (CLI or GUI) to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Enumerate every parked or linked site.
    ListSites,
    /// Register a parked directory. `path` is opaque to `yerd-ipc`;
    /// the daemon canonicalises before storing. Windows paths arrive
    /// with backslashes - that is fine.
    Park {
        /// The directory to park.
        path: PathBuf,
    },
    /// Link a site by name to a directory.
    Link {
        /// The site name (a single DNS label).
        name: String,
        /// The directory to link.
        path: PathBuf,
    },
    /// Remove a linked or parked site by name.
    Unlink {
        /// The site name to remove.
        name: String,
    },
    /// Enumerate the registered parked directory roots (including empty ones,
    /// which produce no sites and so never appear in [`Self::ListSites`]).
    ListParked,
    /// Un-park a directory root: remove it from the parked set and re-scan.
    /// Its parked sites disappear; linked sites are untouched.
    Unpark {
        /// The parked root to remove. Deliberately a `String`, not a
        /// `PathBuf`: the daemon stores parked roots as canonical
        /// `String`s (`config.parked.paths` is a `BTreeSet<String>`), and
        /// clients echo a value straight from [`super::Response::Parked`].
        /// Keeping it a `String` makes the removal an exact identity match -
        /// a `PathBuf` round-trip risks lossy normalisation. The daemon does
        /// **not** canonicalise it (so a folder deleted from disk is still
        /// removable).
        path: String,
    },
    /// Change a site's PHP version.
    SetPhp {
        /// The site name.
        name: String,
        /// The new PHP version.
        version: PhpVersion,
    },
    /// Toggle whether a site is served over HTTPS.
    SetSecure {
        /// The site name.
        name: String,
        /// The desired HTTPS state.
        secure: bool,
    },
    /// Set or clear a site's served web root (the subdirectory served as the
    /// document root, e.g. `public` for Laravel).
    SetWebRoot {
        /// The site name.
        name: String,
        /// The served path. The daemon resolves it against the site's
        /// `document_root` (relative or absolute), validates containment, and
        /// stores the relative remainder. `None` resets the site to
        /// auto-detection.
        path: Option<String>,
    },
    /// Add a routable domain to a site (an exact host or a single-label
    /// wildcard, e.g. `api.foo.test` or `*.foo.test`). The `domain` is the full
    /// FQDN under the configured TLD; the daemon strips the TLD and validates.
    AddDomain {
        /// The site name to add the domain to.
        name: String,
        /// The full domain FQDN (under the configured TLD).
        domain: String,
    },
    /// Remove a routable domain from a site. Removing a site's last exact
    /// (non-wildcard) domain is refused.
    RemoveDomain {
        /// The site name to remove the domain from.
        name: String,
        /// The full domain FQDN to remove.
        domain: String,
    },
    /// Set a site's primary (canonical) domain, the address shown/opened and used
    /// for URL sync. Must be an exact domain; auto-added to the site's set if not
    /// already present.
    SetPrimaryDomain {
        /// The site name.
        name: String,
        /// The full domain FQDN to make primary.
        domain: String,
    },
    /// Reset a site's domains to the default (apex only), clearing any added,
    /// suppressed, or primary customisation.
    ResetDomains {
        /// The site name.
        name: String,
    },
    /// Register a whole-host reverse proxy (`name.test` → `url`).
    AddProxy {
        /// The proxy name (a single DNS label).
        name: String,
        /// The upstream URL, e.g. `http://localhost:8080` (validated by the daemon).
        url: String,
    },
    /// Remove a whole-host reverse proxy by name.
    RemoveProxy {
        /// The proxy name to remove.
        name: String,
    },
    /// Add a path-prefix reverse-proxy rule to an existing site
    /// (`site.test/prefix` → `url`), leaving all other paths served by PHP.
    AddProxyRule {
        /// The site the rule attaches to.
        site: String,
        /// The path prefix, e.g. `/app` (must begin with `/`).
        prefix: String,
        /// The upstream URL (validated by the daemon).
        url: String,
    },
    /// Remove a path-prefix reverse-proxy rule from a site.
    RemoveProxyRule {
        /// The site the rule is on.
        site: String,
        /// The path prefix to remove.
        prefix: String,
    },
    /// Enumerate whole-host proxies and per-site path-prefix rules.
    ListProxies,
    /// Fetch read-only daemon runtime facts (DNS address, TLD, CA path +
    /// fingerprint). Used by `yerd elevate` to drive the privileged helper.
    DaemonInfo,
    /// Download + install a prebuilt PHP version into yerd's data dir.
    InstallPhp {
        /// The major.minor version to install (resolved to a pinned patch).
        version: PhpVersion,
        /// Explicit opt-in to install an out-of-support legacy minor (< 8.2).
        /// The daemon refuses a legacy install unless this is `true`. Defaults
        /// to `false` so older clients (which omit it) can never trigger a
        /// legacy install; skipped-when-false so the existing stable wire
        /// literal stays byte-identical.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        confirm_legacy: bool,
    },
    /// Install a PHP version as a streamed background job. The daemon replies
    /// [`super::Response::JobStarted`] immediately; phase + byte-count progress is
    /// polled via [`Self::JobStatus`]. The streaming sibling of [`Self::InstallPhp`]
    /// (used by the GUI so a multi-minute download shows progress / can cancel).
    InstallPhpStreamed {
        /// The major.minor version to install (resolved to a pinned patch).
        version: PhpVersion,
        /// Explicit opt-in to install a legacy minor; see the `InstallPhp`
        /// variant's `confirm_legacy` for the full contract.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        confirm_legacy: bool,
    },
    /// Set the global default PHP version (terminal `php` shim + site fallback).
    SetDefaultPhp {
        /// The version to make the default; must already be installed.
        version: PhpVersion,
    },
    /// List installed PHP versions and the current default.
    ListPhp,
    /// Upgrade installed PHP to the latest published patch. `Some` = one minor,
    /// `None` = every installed version.
    UpdatePhp {
        /// The minor to update, or `None` for all installed.
        version: Option<PhpVersion>,
    },
    /// Force a poll of the distribution + refresh the update cache, then return
    /// the (enriched) version list.
    CheckPhpUpdates,
    /// List the major.minor versions installable from the distribution (the GUI
    /// install dropdown / `yerd list php --available`). Fetched on demand.
    AvailablePhp,
    /// Merge global PHP ini settings into the config and apply them to all
    /// installed versions' FPM pools. An empty-string value removes a key
    /// (resets it to PHP's built-in default).
    SetPhpSettings {
        /// Setting name → value (e.g. `"memory_limit" -> "512M"`); `""` removes.
        settings: BTreeMap<String, String>,
    },
    /// Merge per-version overrides of the allowlisted PHP ini settings into the
    /// config and apply them to that version's FPM pool and CLI ini. An
    /// empty-string value removes the override (the global value applies again).
    SetPhpVersionSettings {
        /// The installed PHP version the overrides apply to.
        version: PhpVersion,
        /// Setting name → value (e.g. `"memory_limit" -> "1G"`); `""` removes
        /// the per-version override so the global default falls through.
        settings: BTreeMap<String, String>,
    },
    /// Merge free-form per-version ini directives (e.g. `"xdebug.mode" ->
    /// "debug"`) into the config and apply them to that version's FPM pool and
    /// CLI ini. An empty-string value removes the directive.
    SetPhpDirectives {
        /// The installed PHP version the directives apply to.
        version: PhpVersion,
        /// Directive name → value; `""` removes the directive.
        directives: BTreeMap<String, String>,
    },
    /// Register a custom PHP extension for a version: the daemon validates and
    /// load-probes the `.so`, persists it, and loads it into that version's FPM
    /// pool and CLI ini.
    AddPhpExtension {
        /// The PHP version the extension is built for and applies to.
        version: PhpVersion,
        /// Absolute path to the `.so`.
        path: String,
        /// Optional display/removal handle; defaults to the `.so` basename.
        name: Option<String>,
        /// Load as a `zend_extension` rather than a plain `extension`.
        zend: bool,
    },
    /// Remove a registered custom extension by name for a version.
    RemovePhpExtension {
        /// The PHP version the extension is registered under.
        version: PhpVersion,
        /// The extension's registered name.
        name: String,
    },
    /// List registered custom extensions across all versions, each tagged with
    /// whether its `.so` currently exists on disk.
    ListPhpExtensions,
    /// Restart one installed version's FPM pool (stop + ensure).
    RestartPhp {
        /// The version whose pool to restart.
        version: PhpVersion,
    },
    /// Restart every started FPM pool (running or failed).
    RestartAllPhp,
    /// Uninstall an installed PHP version. Blocked when the version is in use by
    /// a site, is the last version while sites remain, or is the current default
    /// while other versions are installed.
    UninstallPhp {
        /// The version to uninstall.
        version: PhpVersion,
    },
    /// Fetch a read-only [`crate::StatusReport`] of daemon/proxy/DNS/PHP health.
    Status,
    /// Run the doctor checks and return the resulting diagnoses.
    Diagnose,
    /// Run the doctor checks, attempt the safe auto-fixes, and report what
    /// happened plus what still needs manual action.
    DoctorFix,
    /// Restart the daemon's own process in place (re-exec). The daemon replies
    /// `Ok` *before* tearing down; the connection then closes as it restarts.
    /// Unix-only.
    RestartDaemon,
    /// List every manageable service with its live status (installed versions,
    /// run state, port, enabled flag).
    ListServices,
    /// List installable vs installed versions per service (the GUI install
    /// dropdown). Fetched on demand from yerd's services distribution.
    AvailableServices,
    /// List `WordPress` core version branches with their PHP compatibility
    /// range (the `WordPress` wizard's core-version dropdown). Sourced from
    /// the hand-maintained `meta/wordpress-versions.json` in the yerd repo,
    /// daemon-side cached; see [`crate::Response::WordpressVersions`].
    AvailableWordpressVersions,
    /// Mint a short-TTL, single-use token for one-click, pre-authenticated
    /// `WordPress` admin login (the "WP Admin" site action). The site must
    /// exist and be detected as `WordPress`; the returned token is consumed by
    /// `yerd-proxy` the moment it's presented on a `/wp-admin` request for
    /// that same site. See [`crate::Response::WordpressLoginToken`].
    MintWordpressLoginToken {
        /// The site name to mint a login token for.
        site: String,
    },
    /// Toggle `WordPress` one-click admin login for a site, and set which
    /// admin user it signs in as.
    SetWordpressAutoLogin {
        /// The site name.
        name: String,
        /// The desired auto-login state.
        enabled: bool,
        /// The `WordPress` login/username to sign in as, or `None` to fall
        /// back to the earliest-created administrator.
        user: Option<String>,
    },
    /// List a `WordPress` site's administrator accounts (the auto-login
    /// user-picker's dropdown). Fetched on demand via `wp user list`.
    WordpressAdminUsers {
        /// The site name.
        site: String,
    },
    /// Download + install a prebuilt service version into yerd's data dir.
    InstallService {
        /// Service id (`"redis"`, `"mysql"`, `"mariadb"`, `"postgres"`).
        service: String,
        /// The version to install.
        version: String,
    },
    /// Uninstall a service version. `purge` also deletes the datadir; without
    /// it the data is retained and its path reported.
    UninstallService {
        /// Service id.
        service: String,
        /// The version to remove.
        version: String,
        /// When true, also delete the engine's datadir (destructive).
        purge: bool,
    },
    /// Start (and enable) a service instance.
    StartService {
        /// Service id.
        service: String,
    },
    /// Stop a service instance. Does not change its autostart preference (use
    /// [`Request::SetServiceAutostart`] for that).
    StopService {
        /// Service id.
        service: String,
    },
    /// Restart a service instance (stop + start).
    RestartService {
        /// Service id.
        service: String,
    },
    /// Change the port a service listens on. Takes effect on the next start /
    /// restart (no implicit hot restart of a live socket).
    SetServicePort {
        /// Service id.
        service: String,
        /// The new loopback port.
        port: u16,
    },
    /// Fetch the last `lines` lines of a service's log file.
    ServiceLogs {
        /// Service id.
        service: String,
        /// How many trailing lines to return.
        lines: u32,
    },
    /// Add a new service instance. For a versioned type (DB/cache) this downloads
    /// the version; for a per-site type (Reverb) it links `site`. Runs as a
    /// background job (reply is [`crate::Response::JobStarted`]) so a slow
    /// download or a failing first start never blocks the daemon.
    AddService {
        /// The service type id (`"redis"`, `"reverb"`, ...).
        type_id: String,
        /// The linked site name, for a per-site type; `None` otherwise.
        site: Option<String>,
        /// An explicit port, or `None` to take the next free one from the type's
        /// default.
        port: Option<u16>,
        /// The version to install, for a versioned type; `None` otherwise.
        version: Option<String>,
        /// Whether the instance starts with Yerd. `None` uses the type's default
        /// (engines start with Yerd; per-site app servers do not).
        autostart: Option<bool>,
    },
    /// Remove a service instance (a per-site instance, or an engine with no
    /// version tracking). `purge` also deletes any on-disk state. Versioned
    /// engines are removed per-version via [`Request::UninstallService`].
    RemoveService {
        /// Instance wire id.
        service: String,
        /// When true, also delete the instance's on-disk state (destructive).
        purge: bool,
    },
    /// Set whether a service instance starts with Yerd (its boot-autostart flag).
    SetServiceAutostart {
        /// Instance wire id.
        service: String,
        /// The desired autostart state.
        enabled: bool,
    },
    /// Re-link a per-site instance (Reverb) to a different site. Changes the
    /// instance's identity; the reply is the new wire id in
    /// [`crate::Response::ServiceInstanceId`].
    SetServiceSite {
        /// Current instance wire id.
        service: String,
        /// The new site to link to.
        site: String,
    },
    /// List the installable service *types* for the "Add Service" dialog (with
    /// per-type multiplicity, install state, versions, and a suggested port). See
    /// [`crate::Response::AddableServices`].
    AddableServiceTypes,
    /// Create a database in a running SQL service (no-op error for caches).
    CreateDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to create (validated as a safe identifier).
        name: String,
    },
    /// List the user databases in a running SQL service (system schemas
    /// filtered out).
    ListDatabases {
        /// Service id (must be a SQL engine).
        service: String,
    },
    /// Drop a database from a running SQL service.
    DropDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to drop (validated; system databases refused).
        name: String,
    },
    /// Back a database up to a plain-SQL file (streamed from the bundled dump tool).
    BackupDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to dump (validated as a safe identifier).
        name: String,
        /// Absolute destination file the daemon writes the dump to. The client
        /// absolutises this against the user's cwd before sending (the daemon's cwd
        /// differs); the path never reaches the dump tool's argv.
        path: PathBuf,
    },
    /// Restore a database from a plain-SQL file (streamed into the bundled client's
    /// stdin). The target database must already exist.
    RestoreDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to restore into (validated; system databases refused).
        name: String,
        /// Absolute source file the daemon reads the dump from. The client
        /// canonicalises this before sending; the path never reaches the client's argv.
        path: PathBuf,
    },
    /// Switch a service to a different version: install `version`, restart the
    /// running instance onto it, then remove the previously-installed version
    /// (the datadir is retained). A service holds one installed version at a
    /// time; this upgrades or downgrades it.
    ChangeServiceVersion {
        /// Service id.
        service: String,
        /// The version to switch to.
        version: String,
    },
    /// Page the buffered dump-telemetry events newer than `since_id` (0 = all),
    /// plus the ids removed since then and the current per-category counts.
    ListDumps {
        /// Return events with `id > since_id`. Clients track the latest id.
        since_id: u64,
    },
    /// Drop every buffered dump event (pinned ones included).
    ClearDumps,
    /// Delete one buffered dump event by id.
    DeleteDump {
        /// The event id to delete.
        id: u64,
    },
    /// Turn dump interception on or off (the "antenna"). Writes the runtime
    /// state file the extension reads; never restarts FPM.
    SetDumpsEnabled {
        /// Desired enabled state.
        enabled: bool,
    },
    /// Set the loopback port the dump server listens on and the extension
    /// connects to.
    SetDumpsPort {
        /// The new loopback port.
        port: u16,
    },
    /// Enable or disable capture of one telemetry feature (e.g. `"queries"`).
    SetDumpFeature {
        /// Feature key (`dumps`/`queries`/`jobs`/`views`/`requests`/`logs`/`cache`).
        feature: String,
        /// Desired enabled state.
        enabled: bool,
    },
    /// Toggle log persistence. `false` (default) clears the buffer on each new
    /// request (latest-request view); `true` accumulates across requests.
    SetDumpsPersist {
        /// Desired persist state.
        persist: bool,
    },
    /// Fetch dump-server status (enabled, port, running, per-version extension
    /// presence, current counts).
    DumpsStatus,
    /// List captured emails (metadata only), newest first.
    ListMails,
    /// Fetch one captured email's full decoded content by id.
    GetMail {
        /// The email id (from [`super::Response::Mails`]).
        id: String,
    },
    /// Delete every captured email.
    ClearMails,
    /// Delete a specific set of captured emails by id (e.g. all the mail shown
    /// for one application). Unknown ids are ignored.
    DeleteMails {
        /// The email ids to delete.
        ids: Vec<String>,
    },
    /// Mark a set of captured emails as read. Unknown ids are ignored.
    MarkMailsRead {
        /// The email ids to mark read.
        ids: Vec<String>,
    },
    /// Set the mail-capture SMTP port. Takes effect on the next daemon
    /// start/restart (no implicit hot rebind), like [`Self::SetServicePort`].
    SetMailPort {
        /// The new loopback port (must be non-zero).
        port: u16,
    },
    /// Set the rootless HTTP/HTTPS fallback ports (the pair the daemon drops to
    /// when 80/443 can't bind without elevation). Both must be `>= 1024` and
    /// differ. Refused while a privileged-port redirect is active (it is pinned
    /// to the current ports). Takes effect on the next daemon restart.
    SetFallbackPorts {
        /// New rootless HTTP port (`>= 1024`).
        http: u16,
        /// New rootless HTTPS port (`>= 1024`).
        https: u16,
    },
    /// Set the embedded DNS responder port (`dns_port`). Must be non-zero. Takes
    /// effect on the next daemon restart (no implicit hot rebind). Changing it may
    /// require re-running the OS-resolver install so it points at the new port.
    SetDnsPort {
        /// The new loopback DNS port (must be non-zero).
        port: u16,
    },
    /// Enable or disable the mail-capture server. Takes effect on the next
    /// daemon start/restart.
    SetMailEnabled {
        /// The desired enabled state.
        enabled: bool,
    },
    /// List the installable dev tools (Composer, Node, Bun) with install status.
    ListTools,
    /// Download + install a dev tool's latest release into yerd's data dir and
    /// expose its commands on `PATH`. Idempotent (reinstalls/updates to latest).
    InstallTool {
        /// Tool id (`"composer"`, `"node"`, `"bun"`).
        tool: String,
    },
    /// Remove a dev tool's files and its `PATH` shims.
    UninstallTool {
        /// Tool id.
        tool: String,
    },
    /// Install a dev tool, streaming its output as a background job. Returns
    /// [`super::Response::JobStarted`] immediately; progress (and the install's
    /// stdout/stderr) is polled via [`Self::JobStatus`]. The streaming sibling of
    /// [`Self::InstallTool`].
    InstallToolStreamed {
        /// Tool id (`"composer"`, `"node"`, `"bun"`, `"laravel"`).
        tool: String,
    },
    /// Scaffold and register a brand-new site (e.g. `laravel new`). Long-running:
    /// the daemon starts a background job and replies [`super::Response::JobStarted`]
    /// immediately; progress is polled via [`Self::JobStatus`].
    CreateSite {
        /// What to create and where.
        spec: crate::CreateSiteSpec,
    },
    /// Poll a running job's progress. `cursor` is the number of log lines the
    /// client has already seen; the daemon returns only newer lines plus the
    /// next cursor. Returns [`super::Response::JobProgress`].
    JobStatus {
        /// The job to poll.
        job_id: crate::JobId,
        /// How many log lines the client already holds.
        cursor: u64,
    },
    /// Request cancellation of a running job (kills its process tree). Returns
    /// [`super::Response::Ok`].
    JobCancel {
        /// The job to cancel.
        job_id: crate::JobId,
    },
    /// Check for an available Yerd self-update. Returns
    /// [`super::Response::UpdateStatus`] reporting the latest stable and edge
    /// versions, the active channel preference, and whether an update is
    /// available. Tolerant of network failure (the daemon serves its cache).
    CheckUpdate {
        /// Override the configured channel for this check only. `None` uses the
        /// persisted `update_channel`.
        channel: Option<crate::Channel>,
    },
    /// Return the **last persisted** self-update result without any network
    /// access - used to pre-fill the UI on load. Returns
    /// [`super::Response::UpdateStatus`] with `source = Cached` and
    /// `checked_at_epoch` set (or, if never checked, the running version with
    /// `checked_at_epoch = None`).
    CachedUpdateStatus,
    /// Persist the self-update channel preference. Returns
    /// [`super::Response::Ok`].
    SetUpdateChannel {
        /// The channel to make the new default.
        channel: crate::Channel,
    },
    /// Download + cryptographically verify the latest update artifact for this
    /// platform on `channel` (the configured channel when `None`). Blocking: the
    /// daemon returns [`super::Response::Staged`] with the on-disk path of the
    /// verified artifact (or [`super::Response::Error`]). The privileged
    /// install/swap is then performed by the applier, not the daemon.
    StageUpdate {
        /// Override the configured channel for this stage only.
        channel: Option<crate::Channel>,
    },
    /// Download + install the `cloudflared` binary as a streamed background job
    /// (the Cloudflare Tunnel integration's prerequisite). Replies
    /// [`super::Response::JobStarted`] immediately; progress is polled via
    /// [`Self::JobStatus`]. The streaming-only sibling of the dev-tool installers.
    InstallCloudflaredStreamed,
    /// Start a Quick Tunnel for a site, publishing it at a random
    /// `*.trycloudflare.com` URL. Replies [`super::Response::Tunnels`] with the
    /// live tunnel (including its URL once captured). Requires `cloudflared` to be
    /// installed.
    StartQuickTunnel {
        /// The site name to share.
        site: String,
    },
    /// Stop and tear down the tunnel for a site. No-op if none is running.
    StopTunnel {
        /// The site whose tunnel to stop.
        site: String,
    },
    /// Fetch the live tunnel state plus `cloudflared` install status. Returns
    /// [`super::Response::Tunnels`].
    TunnelStatus,
    /// Run the interactive Cloudflare account login (`cloudflared tunnel login`)
    /// as a streamed background job. The job log carries the one-time auth URL
    /// line for the GUI to open. Replies [`super::Response::JobStarted`]. Named
    /// Tunnels (Phase 2).
    CloudflaredLogin,
    /// Create a named tunnel on the logged-in account, recording its UUID.
    /// Replies [`super::Response::Ok`] (or `Error`). Requires a prior login.
    CreateNamedTunnel {
        /// The tunnel name to create.
        name: String,
    },
    /// List the named tunnels recorded locally. Returns
    /// [`super::Response::NamedTunnels`].
    ListNamedTunnels,
    /// Route a DNS hostname to a named tunnel (`cloudflared tunnel route dns`),
    /// creating the proxied CNAME on the user's Cloudflare zone. Account- and
    /// DNS-mutating; replies [`super::Response::Ok`].
    RouteTunnelDns {
        /// The tunnel name (or UUID) to route to.
        tunnel: String,
        /// The public hostname to create.
        hostname: String,
    },
    /// Set or clear a site's persisted public hostname (the named-tunnel
    /// mapping). Setting a hostname enables the site in the named tunnel;
    /// `None` removes (disables) it. Replies [`super::Response::Ok`].
    SetSiteTunnel {
        /// The site name.
        site: String,
        /// The public hostname, or `None` to remove the mapping.
        hostname: Option<String>,
    },
    /// (Re)start the single consolidated Named Tunnel serving every enabled site
    /// (one process, one config with one ingress rule per site). Returns
    /// [`super::Response::Tunnels`].
    StartNamedTunnel,
    /// Stop the consolidated Named Tunnel. Returns [`super::Response::Tunnels`].
    StopNamedTunnel,
    /// Delete a named tunnel from the Cloudflare account and forget it locally
    /// (stops the process, removes its credentials, and clears the persisted
    /// tunnel/site mappings). Account-mutating; replies [`super::Response::Ok`].
    DeleteNamedTunnel {
        /// The tunnel name to delete.
        name: String,
    },
    /// List the user-defined site groups (ordered) and per-site membership.
    /// Returns [`super::Response::Groups`]. Groups are a GUI organisational
    /// overlay and do not affect routing.
    ListGroups,
    /// Create a new site group, appended last in display order. Replies
    /// [`super::Response::Ok`]. Rejected if the name is empty, a duplicate
    /// (case-insensitive), or the reserved `Unallocated`.
    CreateGroup {
        /// The group display name to create.
        name: String,
    },
    /// Delete a site group. Its member sites fall back to the synthetic
    /// "Unallocated" bucket (their membership entries are dropped). Replies
    /// [`super::Response::Ok`].
    DeleteGroup {
        /// The group name to delete.
        name: String,
    },
    /// Replace the group display order. `order` must be an exact permutation of
    /// the existing group names. Replies [`super::Response::Ok`].
    SetGroupOrder {
        /// The full set of group names in the desired display order.
        order: Vec<String>,
    },
    /// Set or clear a site's group membership (a site belongs to at most one
    /// group). `Some(group)` must name an existing group; `None` moves the site
    /// to "Unallocated". Replies [`super::Response::Ok`].
    SetSiteGroup {
        /// The site name.
        site: String,
        /// The group to assign, or `None` to unassign.
        group: Option<String>,
    },
    /// Rename a site group, preserving its display position and moving every
    /// member with it. Replies [`super::Response::Ok`]. Rejected if `to` is
    /// empty, the reserved `Unallocated`, or a case-insensitive duplicate of a
    /// different group, or if `from` names no group.
    RenameGroup {
        /// The current group name.
        from: String,
        /// The new group name.
        to: String,
    },
    /// Enable or disable the proxy's symlink-escape protection (the global
    /// `symlink_protection` setting). When disabled, the proxy serves assets
    /// and resolves scripts reached via a symlink that resolves outside a
    /// site's document root. Takes effect immediately (no daemon restart) and
    /// is persisted to config.
    SetSymlinkProtection {
        /// `true` = protection on (block escapes); `false` = allow escapes.
        enabled: bool,
    },
    /// Override a site's front-controller mode. When enabled, every request
    /// funnels through the site-root `index.php`; when disabled, a named `.php`
    /// under the served root is executed directly. Persisted per site and
    /// applied on the next request. See
    /// [`yerd_core::Site::uses_front_controller`].
    SetFrontController {
        /// The site name.
        name: String,
        /// `true` = front-controller mode; `false` = direct script execution.
        enabled: bool,
    },
    /// Enable or disable the MCP server gate (whether `yerd mcp` serves tools
    /// to local AI agents). Persisted to config and reported back through
    /// [`crate::StatusReport::mcp_enabled`]; the daemon itself runs no MCP
    /// server, so this only gates `yerd mcp` sessions.
    SetMcpEnabled {
        /// `true` = agents may call Yerd's MCP tools; `false` = gated off.
        enabled: bool,
    },
    /// Enable or disable LAN exposure (serving `.test` sites to other devices on
    /// the network). Persisted to config; the actual re-bind happens on the
    /// daemon restart the CLI triggers next, so this is persist-only here.
    /// Reported back via [`crate::StatusReport::lan_enabled`].
    SetLanEnabled {
        /// `true` = expose to the LAN; `false` = loopback-only.
        enabled: bool,
    },
    /// Mint a one-time, expiring code for the remote-device bootstrap and return
    /// the setup URL + the CA fingerprint (for out-of-band verification). Only
    /// valid while LAN mode is up; otherwise the daemon returns an error.
    MintRemoteSetupCode,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::match_same_arms
)]
mod variant_name_pinning {
    use super::*;
    use std::path::PathBuf;

    // Inline (not in tests/) so the #[non_exhaustive] enum matches
    // exhaustively: a renamed Rust variant fails this match at compile time.
    #[allow(dead_code, clippy::too_many_lines)]
    fn pin(r: Request) {
        match r {
            Request::Ping => {}
            Request::ListSites => {}
            Request::Park { .. } => {}
            Request::Link { .. } => {}
            Request::Unlink { .. } => {}
            Request::ListParked => {}
            Request::Unpark { .. } => {}
            Request::SetPhp { .. } => {}
            Request::SetSecure { .. } => {}
            Request::SetWebRoot { .. } => {}
            Request::AddDomain { .. } => {}
            Request::RemoveDomain { .. } => {}
            Request::SetPrimaryDomain { .. } => {}
            Request::ResetDomains { .. } => {}
            Request::DaemonInfo => {}
            Request::InstallPhp { .. } => {}
            Request::InstallPhpStreamed { .. } => {}
            Request::SetDefaultPhp { .. } => {}
            Request::ListPhp => {}
            Request::UpdatePhp { .. } => {}
            Request::CheckPhpUpdates => {}
            Request::AvailablePhp => {}
            Request::SetPhpSettings { .. } => {}
            Request::SetPhpVersionSettings { .. } => {}
            Request::SetPhpDirectives { .. } => {}
            Request::AddPhpExtension { .. } => {}
            Request::RemovePhpExtension { .. } => {}
            Request::ListPhpExtensions => {}
            Request::RestartPhp { .. } => {}
            Request::RestartAllPhp => {}
            Request::UninstallPhp { .. } => {}
            Request::Status => {}
            Request::Diagnose => {}
            Request::DoctorFix => {}
            Request::RestartDaemon => {}
            Request::ListServices => {}
            Request::AvailableServices => {}
            Request::AvailableWordpressVersions => {}
            Request::MintWordpressLoginToken { .. } => {}
            Request::SetWordpressAutoLogin { .. } => {}
            Request::WordpressAdminUsers { .. } => {}
            Request::InstallService { .. } => {}
            Request::UninstallService { .. } => {}
            Request::StartService { .. } => {}
            Request::StopService { .. } => {}
            Request::RestartService { .. } => {}
            Request::SetServicePort { .. } => {}
            Request::ServiceLogs { .. } => {}
            Request::AddService { .. } => {}
            Request::RemoveService { .. } => {}
            Request::SetServiceAutostart { .. } => {}
            Request::SetServiceSite { .. } => {}
            Request::AddableServiceTypes => {}
            Request::CreateDatabase { .. } => {}
            Request::ListDatabases { .. } => {}
            Request::DropDatabase { .. } => {}
            Request::BackupDatabase { .. } => {}
            Request::RestoreDatabase { .. } => {}
            Request::ChangeServiceVersion { .. } => {}
            Request::ListDumps { .. } => {}
            Request::ClearDumps => {}
            Request::DeleteDump { .. } => {}
            Request::SetDumpsEnabled { .. } => {}
            Request::SetDumpsPort { .. } => {}
            Request::SetDumpFeature { .. } => {}
            Request::SetDumpsPersist { .. } => {}
            Request::DumpsStatus => {}
            Request::ListMails => {}
            Request::GetMail { .. } => {}
            Request::ClearMails => {}
            Request::DeleteMails { .. } => {}
            Request::MarkMailsRead { .. } => {}
            Request::SetMailPort { .. } => {}
            Request::SetFallbackPorts { .. } => {}
            Request::SetDnsPort { .. } => {}
            Request::SetMailEnabled { .. } => {}
            Request::ListTools => {}
            Request::InstallTool { .. } => {}
            Request::UninstallTool { .. } => {}
            Request::InstallToolStreamed { .. } => {}
            Request::CreateSite { .. } => {}
            Request::JobStatus { .. } => {}
            Request::JobCancel { .. } => {}
            Request::CheckUpdate { .. } => {}
            Request::CachedUpdateStatus => {}
            Request::SetUpdateChannel { .. } => {}
            Request::StageUpdate { .. } => {}
            Request::InstallCloudflaredStreamed => {}
            Request::StartQuickTunnel { .. } => {}
            Request::StopTunnel { .. } => {}
            Request::TunnelStatus => {}
            Request::CloudflaredLogin => {}
            Request::CreateNamedTunnel { .. } => {}
            Request::ListNamedTunnels => {}
            Request::RouteTunnelDns { .. } => {}
            Request::SetSiteTunnel { .. } => {}
            Request::StartNamedTunnel => {}
            Request::StopNamedTunnel => {}
            Request::DeleteNamedTunnel { .. } => {}
            Request::ListGroups => {}
            Request::CreateGroup { .. } => {}
            Request::DeleteGroup { .. } => {}
            Request::SetGroupOrder { .. } => {}
            Request::SetSiteGroup { .. } => {}
            Request::RenameGroup { .. } => {}
            Request::SetSymlinkProtection { .. } => {}
            Request::SetFrontController { .. } => {}
            Request::AddProxy { .. } => {}
            Request::RemoveProxy { .. } => {}
            Request::AddProxyRule { .. } => {}
            Request::RemoveProxyRule { .. } => {}
            Request::ListProxies => {}
            Request::SetMcpEnabled { .. } => {}
            Request::SetLanEnabled { .. } => {}
            Request::MintRemoteSetupCode => {}
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn touch_every_variant() {
        pin(Request::Ping);
        pin(Request::ListSites);
        pin(Request::Park {
            path: PathBuf::from("/x"),
        });
        pin(Request::Link {
            name: "x".into(),
            path: PathBuf::from("/x"),
        });
        pin(Request::Unlink { name: "x".into() });
        pin(Request::ListParked);
        pin(Request::Unpark { path: "/x".into() });
        pin(Request::SetPhp {
            name: "x".into(),
            version: PhpVersion::new(8, 3),
        });
        pin(Request::SetSecure {
            name: "x".into(),
            secure: true,
        });
        pin(Request::SetWebRoot {
            name: "x".into(),
            path: Some("public".into()),
        });
        pin(Request::AddDomain {
            name: "foo".into(),
            domain: "api.foo.test".into(),
        });
        pin(Request::RemoveDomain {
            name: "foo".into(),
            domain: "api.foo.test".into(),
        });
        pin(Request::SetPrimaryDomain {
            name: "foo".into(),
            domain: "corp.test".into(),
        });
        pin(Request::ResetDomains { name: "foo".into() });
        pin(Request::DaemonInfo);
        pin(Request::InstallPhp {
            version: PhpVersion::new(8, 5),
            confirm_legacy: false,
        });
        pin(Request::SetDefaultPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::ListPhp);
        pin(Request::UpdatePhp {
            version: Some(PhpVersion::new(8, 5)),
        });
        pin(Request::CheckPhpUpdates);
        pin(Request::AvailablePhp);
        pin(Request::SetPhpSettings {
            settings: BTreeMap::new(),
        });
        pin(Request::AddPhpExtension {
            version: PhpVersion::new(8, 5),
            path: "/a/scrypt.so".into(),
            name: None,
            zend: false,
        });
        pin(Request::RemovePhpExtension {
            version: PhpVersion::new(8, 5),
            name: "scrypt".into(),
        });
        pin(Request::ListPhpExtensions);
        pin(Request::RestartPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::RestartAllPhp);
        pin(Request::UninstallPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::Status);
        pin(Request::Diagnose);
        pin(Request::DoctorFix);
        pin(Request::RestartDaemon);
        pin(Request::ListServices);
        pin(Request::AvailableServices);
        pin(Request::AvailableWordpressVersions);
        pin(Request::MintWordpressLoginToken {
            site: "blog".into(),
        });
        pin(Request::SetWordpressAutoLogin {
            name: "blog".into(),
            enabled: true,
            user: Some("admin".into()),
        });
        pin(Request::WordpressAdminUsers {
            site: "blog".into(),
        });
        pin(Request::InstallService {
            service: "redis".into(),
            version: "8".into(),
        });
        pin(Request::UninstallService {
            service: "redis".into(),
            version: "8".into(),
            purge: false,
        });
        pin(Request::StartService {
            service: "redis".into(),
        });
        pin(Request::StopService {
            service: "redis".into(),
        });
        pin(Request::RestartService {
            service: "redis".into(),
        });
        pin(Request::SetServicePort {
            service: "redis".into(),
            port: 6380,
        });
        pin(Request::ServiceLogs {
            service: "redis".into(),
            lines: 100,
        });
        pin(Request::CreateDatabase {
            service: "mysql".into(),
            name: "app".into(),
        });
        pin(Request::ListDatabases {
            service: "mysql".into(),
        });
        pin(Request::DropDatabase {
            service: "mysql".into(),
            name: "app".into(),
        });
        pin(Request::BackupDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("/x/app.sql"),
        });
        pin(Request::RestoreDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("/x/app.sql"),
        });
        pin(Request::ChangeServiceVersion {
            service: "redis".into(),
            version: "9.1.0".into(),
        });
        pin(Request::ListDumps { since_id: 0 });
        pin(Request::ClearDumps);
        pin(Request::DeleteDump { id: 1 });
        pin(Request::SetDumpsEnabled { enabled: true });
        pin(Request::SetDumpsPort { port: 2304 });
        pin(Request::SetDumpFeature {
            feature: "queries".into(),
            enabled: true,
        });
        pin(Request::SetDumpsPersist { persist: true });
        pin(Request::DumpsStatus);
        pin(Request::ListMails);
        pin(Request::GetMail {
            id: "000001".into(),
        });
        pin(Request::ClearMails);
        pin(Request::DeleteMails {
            ids: vec!["000001".into()],
        });
        pin(Request::MarkMailsRead {
            ids: vec!["000001".into()],
        });
        pin(Request::SetMailPort { port: 2525 });
        pin(Request::SetFallbackPorts {
            http: 8080,
            https: 8443,
        });
        pin(Request::SetMailEnabled { enabled: true });
        pin(Request::ListTools);
        pin(Request::InstallTool {
            tool: "node".into(),
        });
        pin(Request::UninstallTool {
            tool: "node".into(),
        });
        pin(Request::InstallToolStreamed {
            tool: "laravel".into(),
        });
        pin(Request::CreateSite {
            spec: crate::CreateSiteSpec {
                name: "blog".into(),
                parent_dir: PathBuf::from("/srv"),
                php: PhpVersion::new(8, 4),
                secure: true,
                framework: crate::Framework::Laravel {
                    options: laravel_options_fixture(),
                },
            },
        });
        pin(Request::JobStatus {
            job_id: "j1".into(),
            cursor: 0,
        });
        pin(Request::JobCancel {
            job_id: "j1".into(),
        });
        pin(Request::CheckUpdate {
            channel: Some(crate::Channel::Edge),
        });
        pin(Request::CachedUpdateStatus);
        pin(Request::SetUpdateChannel {
            channel: crate::Channel::Stable,
        });
        pin(Request::StageUpdate { channel: None });
        pin(Request::InstallCloudflaredStreamed);
        pin(Request::StartQuickTunnel { site: "app".into() });
        pin(Request::StopTunnel { site: "app".into() });
        pin(Request::TunnelStatus);
        pin(Request::CloudflaredLogin);
        pin(Request::CreateNamedTunnel {
            name: "mysite".into(),
        });
        pin(Request::ListNamedTunnels);
        pin(Request::RouteTunnelDns {
            tunnel: "mysite".into(),
            hostname: "app.example.com".into(),
        });
        pin(Request::SetSiteTunnel {
            site: "app".into(),
            hostname: Some("app.example.com".into()),
        });
        pin(Request::StartNamedTunnel);
        pin(Request::StopNamedTunnel);
        pin(Request::DeleteNamedTunnel {
            name: "mysite".into(),
        });
        pin(Request::ListGroups);
        pin(Request::CreateGroup {
            name: "Blog".into(),
        });
        pin(Request::DeleteGroup {
            name: "Blog".into(),
        });
        pin(Request::SetGroupOrder {
            order: vec!["Blog".into(), "Shop".into()],
        });
        pin(Request::SetSiteGroup {
            site: "app".into(),
            group: Some("Blog".into()),
        });
        pin(Request::RenameGroup {
            from: "Blog".into(),
            to: "Journal".into(),
        });
        pin(Request::SetSymlinkProtection { enabled: true });
        pin(Request::SetFrontController {
            name: "blog".to_owned(),
            enabled: true,
        });
        pin(Request::AddProxy {
            name: "reverb".to_owned(),
            url: "http://localhost:8080".to_owned(),
        });
        pin(Request::RemoveProxy {
            name: "reverb".to_owned(),
        });
        pin(Request::AddProxyRule {
            site: "app".to_owned(),
            prefix: "/app".to_owned(),
            url: "http://127.0.0.1:8080".to_owned(),
        });
        pin(Request::RemoveProxyRule {
            site: "app".to_owned(),
            prefix: "/app".to_owned(),
        });
        pin(Request::ListProxies);
        pin(Request::SetMcpEnabled { enabled: true });
        pin(Request::SetLanEnabled { enabled: true });
        pin(Request::MintRemoteSetupCode);
    }

    fn laravel_options_fixture() -> crate::LaravelOptions {
        crate::LaravelOptions {
            starter_kit: crate::StarterKit::React,
            auth: crate::AuthProvider::Laravel,
            livewire_class_components: false,
            teams: false,
            testing: crate::Testing::Pest,
            database: crate::Database::Sqlite,
            js: crate::JsRuntime::Npm,
            git: true,
            boost: false,
        }
    }
}
