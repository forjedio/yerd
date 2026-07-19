//! Status (`yerd status`) and doctor (`yerd doctor`) payload types.
//!
//! These travel inside [`crate::Response::Status`],
//! [`crate::Response::Diagnoses`], and [`crate::Response::DoctorFix`]. As with
//! the rest of this crate they are a published contract: add fields/variants
//! additively, never rename, and let `rename_all` (never per-field renames)
//! handle casing. `tests/wire_stability.rs` pins the byte-exact shape.
//!
//! ## No `f64` on the wire
//!
//! [`crate::Response`] derives `Eq`, so nothing reachable from it may contain a
//! float. The system load average therefore crosses as integer hundredths
//! ([`StatusReport::load_avg`] = `load × 100`); the CLI renders it back to
//! `x.xx`. The daemon computes the conversion from the platform layer's `f64`
//! reading at assembly time.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::PhpVersion;

/// A read-only snapshot of daemon runtime health, returned for
/// [`crate::Request::Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The daemon's process id.
    pub daemon_pid: u32,
    /// Seconds since the daemon finished starting up.
    pub uptime_secs: u64,
    /// Resident memory of the daemon process, in bytes. The reverse proxy and
    /// the DNS responder run as tasks inside this process, so this single figure
    /// covers all three. `None` where unavailable (non-Linux, or a transient
    /// read failure).
    pub daemon_rss_bytes: Option<u64>,
    /// The TLD served (e.g. `"test"`).
    pub tld: String,
    /// HTTP listener: requested vs bound port.
    pub http: PortStatus,
    /// HTTPS listener: requested vs bound port.
    pub https: PortStatus,
    /// Address the embedded DNS responder is bound on.
    pub dns_addr: SocketAddr,
    /// Local CA facts, including its system-store trust state.
    pub ca: CaStatus,
    /// Whether the OS resolver routes `*.<tld>` to Yerd. `None` = the probe
    /// could not determine it (treat as "unknown", **not** as `false`).
    pub resolver_installed: Option<bool>,
    /// Whether a privileged-port redirect is active so 80/443 reach the
    /// daemon's rootless ports (macOS pf `rdr`). `Some(true)` lets the doctor
    /// treat a port *fallback* as satisfied; `None` = not applicable (Linux,
    /// where elevation binds 80/443 directly) or undeterminable. `#[serde(default,
    /// skip_serializing_if)]` keeps the Linux wire bytes unchanged and older
    /// daemons decodable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_redirect: Option<bool>,
    /// `Some(true)` when a privileged web port (80/443) is held by a listener
    /// that is **not** this daemon's proxy - a foreign process (or stale `pf`
    /// rule) squatting the port Yerd wants. Confirmed via the proxy's `Server:`
    /// marker, so it never mistakes Yerd for a foreign listener. `Some(false)` =
    /// no conflict; `None` = not probed. Cross-platform (unlike `port_redirect`).
    /// `#[serde(default, skip_serializing_if)]` keeps the wire additive for older
    /// daemons/clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_web_listener: Option<bool>,
    /// If installing the OS resolver replaced a pre-existing `/etc/resolver/<tld>`
    /// (e.g. a Valet/Herd leftover), the absolute path of the timestamped backup
    /// Yerd saved - surfaced as an informational `doctor` finding. `None` when
    /// nothing was replaced (or the backup is older than the daemon's reporting
    /// window). macOS-only; omitted from the wire when `None` (`#[serde(default,
    /// skip_serializing_if)]`) so Linux/older clients are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver_backup: Option<String>,
    /// The global default PHP version.
    pub default_php: PhpVersion,
    /// One entry per installed PHP version, with live FPM state.
    pub php: Vec<PhpPoolStatus>,
    /// Site counts by kind.
    pub sites: SiteCounts,
    /// System load average for 1/5/15 minutes, each `× 100` (hundredths).
    /// `None` where unavailable (non-Linux, or a transient read failure).
    pub load_avg: Option<[u32; 3]>,
    /// The daemon's own version (its `CARGO_PKG_VERSION`, e.g. `"2.0.1"`).
    /// `#[serde(default)]` so a newer client decoding an *older* daemon's status
    /// (which lacks this key) gets `""` and renders "unknown" rather than failing
    /// the whole decode - the daemon/GUI version skew this field exists to show.
    /// The daemon always sets a non-empty value, so it is always emitted.
    #[serde(default)]
    pub daemon_version: String,
    /// Per-service status (databases / caches). `#[serde(default,
    /// skip_serializing_if)]` keeps the wire additive: an older daemon (no
    /// services) emits unchanged bytes and an older client decodes a newer
    /// daemon by ignoring nothing (the field simply defaults to empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceStatus>,
    /// Built-in mail-capture server status. `None` when the daemon predates the
    /// feature; `#[serde(default, skip_serializing_if)]` keeps the wire additive
    /// (an older daemon emits unchanged bytes; an older client ignores it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<MailStatus>,
    /// Set when the daemon could bind **neither** the desired nor the rootless
    /// fallback web port pair: it runs degraded (IPC/DNS up, no HTTP/HTTPS
    /// proxy). Carries the fallback ports it failed on so the UI/doctor can name
    /// them. `None`/absent on a healthy daemon; `#[serde(default,
    /// skip_serializing_if)]` keeps the wire additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_unbound: Option<UnboundWeb>,
    /// Set when the daemon could not bind its DNS responder port (the configured
    /// `dns_port`): it runs degraded (HTTP/HTTPS/IPC up, but `*.test` names won't
    /// resolve through Yerd) rather than aborting. Carries the configured port it
    /// failed on so the UI/doctor can name it. `None`/absent on a healthy daemon;
    /// `#[serde(default, skip_serializing_if)]` keeps the wire additive. Mirrors
    /// [`StatusReport::web_unbound`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_unbound: Option<u16>,
    /// A random id the daemon generates once per process at startup. Clients use
    /// a *change* in this value to detect that a restart actually completed (the
    /// re-exec preserves the PID and `uptime_secs` has only one-second
    /// granularity, so neither is a reliable restart key). `None`/absent on an
    /// older daemon; `#[serde(default, skip_serializing_if)]` keeps the wire
    /// additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<u64>,
    /// Number of sites currently shared to the public internet: live quick
    /// tunnels plus, when the named tunnel is running, the sites it exposes. `0`
    /// when nothing is shared. `#[serde(default, skip_serializing_if)]` keeps the
    /// wire additive (an older daemon emits unchanged bytes; an older client
    /// ignores it).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub shared_sites: u32,
    /// Whether the proxy's symlink-escape protection is on (the global
    /// `symlink_protection` setting). `true` = protection active (block symlinks
    /// resolving outside a site's document root); `false` = the user has opted
    /// out. `#[serde(default = "default_true")]` so a *newer* client decoding an
    /// *older* daemon's status (which lacks this key) reads it as protected
    /// rather than silently off. The daemon always emits it.
    #[serde(default = "default_true")]
    pub symlink_protection: bool,
    /// Sites that lost a domain to another site during routing: a domain (the
    /// apex, or a hand-edited explicit domain) claimed by more than one site is
    /// dropped from the loser when the router is built. Empty on a healthy
    /// config. `#[serde(default, skip_serializing_if)]` keeps the wire additive -
    /// an older daemon emits unchanged bytes, an older client ignores it.
    /// Surfaced by `yerd doctor`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<DomainShadow>,
    /// Whether the MCP server gate is on, i.e. whether `yerd mcp` serves tools
    /// to local AI agents. `#[serde(default)]` so a newer client decoding an
    /// *older* daemon's status (which lacks this key) reads the opt-in default
    /// of off rather than failing the decode. The daemon always emits it.
    #[serde(default)]
    pub mcp_enabled: bool,
    /// Whether LAN exposure is enabled in the daemon's *config* (the configured
    /// state). Compare with the effective signals ([`Self::lan_ip`],
    /// [`Self::lan_setup_bound`]) to detect "enabled but not yet applied".
    /// `#[serde(default)]` keeps the wire additive; the daemon always emits it.
    #[serde(default)]
    pub lan_enabled: bool,
    /// The host's discovered LAN IPv4 when LAN mode is on and discovery
    /// succeeded; `None` otherwise. `skip_serializing_if` keeps the absent bytes
    /// unchanged for older daemons/clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_ip: Option<std::net::Ipv4Addr>,
    /// Whether the remote-setup bootstrap listener actually bound (effective
    /// state). `None` when LAN is off (nothing to report); `Some(false)` when LAN
    /// is on but the listener couldn't bind (degraded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_setup_bound: Option<bool>,
}

/// One shadow relationship, surfaced in [`StatusReport::shadows`] and by `yerd
/// doctor`: a domain `site` wanted is instead routed to `shadowed_by`. The
/// common case is a shadowed apex; a hand-edited config can also collide two
/// sites on an explicit domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainShadow {
    /// The site that lost the domain.
    pub site: String,
    /// The other site that claims it.
    pub shadowed_by: String,
}

/// `skip_serializing_if` helper: a `u32` that is zero is omitted from the wire.
/// Takes `&u32` because that is the signature serde's `skip_serializing_if`
/// requires.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// `serde` default for [`StatusReport::symlink_protection`]: protection on, so
/// an older daemon's status (missing the key) reads as protected, not off.
fn default_true() -> bool {
    true
}

/// The rootless fallback ports the daemon failed to bind, surfaced in
/// [`StatusReport::web_unbound`] when it runs degraded (no web listeners).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnboundWeb {
    /// The configured rootless HTTP port that could not be bound.
    pub http: u16,
    /// The configured rootless HTTPS port that could not be bound.
    pub https: u16,
}

/// Built-in mail-capture SMTP server status, surfaced in [`StatusReport::mail`].
///
/// Integer/bool only so the enclosing `Response` keeps its `Eq` derive.
/// `enabled` and `listening` are distinct: `enabled && !listening` means the
/// user turned mail on but the port could not be bound (e.g. already in use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailStatus {
    /// Whether mail capture is enabled in the config.
    pub enabled: bool,
    /// The configured loopback SMTP port.
    pub port: u16,
    /// Whether the capture server actually bound the port and is accepting mail.
    pub listening: bool,
    /// Number of captured emails currently stored on disk.
    pub count: u32,
    /// Number of captured emails not yet marked read. `#[serde(default)]` so an
    /// older daemon (no `unread`) decodes as `0`.
    #[serde(default)]
    pub unread: u32,
}

/// One captured email's metadata, returned in [`crate::Response::Mails`].
///
/// String/integer only (no `Cow`/lifetimes, no floats) - fully owned so it
/// crosses the wire and keeps the enclosing `Response`'s `Eq` derive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailSummary {
    /// Opaque stable id (also the on-disk `<id>.eml` stem).
    pub id: String,
    /// The `From:` header, raw display form (e.g. `Example <hello@example.com>`).
    pub from: String,
    /// The `To:` recipients, raw display form, one per address.
    pub to: Vec<String>,
    /// The `Subject:` header (empty when absent).
    pub subject: String,
    /// The message `Date:` as Unix epoch seconds; `0` when unparseable/absent.
    pub date_epoch: u64,
    /// Whether the email has been marked read. `#[serde(default)]` so an older
    /// `index.json` (no `read`) decodes as `false` (unread).
    #[serde(default)]
    pub read: bool,
}

/// A single decoded header line, for [`MailDetail::headers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailHeader {
    /// Header field name (e.g. `Content-Type`).
    pub name: String,
    /// Header field value, decoded to UTF-8.
    pub value: String,
}

/// One captured email's full decoded content, returned in
/// [`crate::Response::Mail`]. The bodies are already MIME-decoded (charset +
/// transfer-encoding) and the HTML body has had `cid:` images rewritten to
/// `data:` URLs, so a client can render it directly in a sandboxed frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailDetail {
    /// Opaque stable id (matches the [`MailSummary::id`]).
    pub id: String,
    /// The `From:` header, raw display form.
    pub from: String,
    /// The `To:` recipients, raw display form.
    pub to: Vec<String>,
    /// The `Subject:` header (empty when absent).
    pub subject: String,
    /// The message `Date:` as Unix epoch seconds; `0` when unparseable/absent.
    pub date_epoch: u64,
    /// All header lines, in the order they appeared.
    pub headers: Vec<MailHeader>,
    /// Decoded `text/html` body, when the message has one.
    pub html_body: Option<String>,
    /// Decoded `text/plain` body, when the message has one.
    pub text_body: Option<String>,
}

/// A listener's requested vs actually-bound port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortStatus {
    /// The port the config asked for.
    pub requested: u16,
    /// The port actually bound (differs from `requested` on rootless fallback).
    pub bound: u16,
    /// `true` when `bound != requested` (a rootless fallback fired).
    pub fell_back: bool,
}

/// Local CA facts surfaced in [`StatusReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaStatus {
    /// Absolute path to the CA certificate PEM.
    pub path: PathBuf,
    /// SHA-256 fingerprint, 64 lowercase hex chars.
    pub fingerprint: String,
    /// Whether the CA is **effectively trusted** for SSL by the OS - not merely
    /// present in a store. macOS evaluates the user/admin/system trust domains
    /// (`security verify-cert`); Linux treats anchor-dir presence as trust.
    /// `None` = the probe could not determine it (**not** `false`).
    pub trusted_system: Option<bool>,
    /// Whether the **bundled PHP** trusts the Yerd CA: the managed
    /// `{data}/cacert.pem` exists and contains the CA. `Some(false)` means the
    /// bundle is missing/stale (PHP HTTPS to `.test` fails); `None` = the
    /// feature is off (no host roots found) or the probe could not run.
    /// `#[serde(default, skip_serializing_if)]` keeps the wire additive for
    /// older clients/daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php_trusts_ca: Option<bool>,
}

/// Site counts by kind, for [`StatusReport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCounts {
    /// Number of parked sites.
    pub parked: usize,
    /// Number of linked sites.
    pub linked: usize,
    /// Number of sites served over HTTPS.
    pub secured: usize,
}

/// Per-version PHP-FPM status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpPoolStatus {
    /// The installed minor version.
    pub version: PhpVersion,
    /// The installed full patch (e.g. `"8.5.6"`), if recorded.
    pub installed_patch: Option<String>,
    /// Live FPM run state for this version.
    pub state: PoolRunState,
    /// FPM master PID when running.
    pub pid: Option<u32>,
    /// FPM listen address (socket path, or `127.0.0.1:<port>`) when running.
    pub listen: Option<String>,
    /// Resident memory of the FPM master in bytes, when measurable.
    pub rss_bytes: Option<u64>,
    /// Newest published patch when newer than installed (from the update cache).
    pub update_available: Option<String>,
}

/// Live FPM run state for a single PHP version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PoolRunState {
    /// FPM is supervised and its master process is alive.
    Running,
    /// No FPM pool for this version (installed but never started, or stopped).
    Stopped,
    /// A supervised pool's master process has died.
    Failed,
}

/// Live run state of a supervised service. Mirrors [`PoolRunState`] but is a
/// distinct type so services and PHP pools can evolve independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServiceRunState {
    /// The server process is supervised and alive.
    Running,
    /// Not running (installed but never started, or stopped).
    Stopped,
    /// A supervised server process has died.
    Failed,
}

/// Per-service status snapshot, returned in [`crate::Response::Services`] and in
/// [`StatusReport::services`]. Every field is integer-only / string / bool so
/// the enclosing `Response` keeps its `Eq` derive (no floats on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Stable service id (`"redis"`, `"mysql"`, `"mariadb"`, `"postgres"`, `"meilisearch"`).
    pub service: String,
    /// Human-facing label (`"Redis (Valkey)"`, `"PostgreSQL"`, …).
    pub display_name: String,
    /// Versions installed on disk, ascending. Empty when the engine is not
    /// installed.
    pub installed_versions: Vec<String>,
    /// The configured/selected version, if the user has chosen one.
    pub selected_version: Option<String>,
    /// Live run state.
    pub state: ServiceRunState,
    /// Server PID when running.
    pub pid: Option<u32>,
    /// Listen address (`"127.0.0.1:6379"`) when running.
    pub listen: Option<String>,
    /// The effective (configured or default) port.
    pub port: u16,
    /// Whether the daemon auto-starts this instance on boot.
    pub enabled: bool,
    /// Whether the engine hosts SQL databases (gates "Create Database" in the GUI).
    pub supports_databases: bool,
    /// The service *type* id (`"redis"`, `"reverb"`), distinct from `service`
    /// (the instance wire id, e.g. `"reverb:blog"`). Additive: older daemons omit
    /// it, so a client falls back to `service` when this is empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub type_id: String,
    /// The linked site name for a per-site instance (`Some("blog")`); `None` for
    /// a single-instance engine. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// The last failure message when `state` is `Failed`, for display in the UI;
    /// `None` otherwise. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One installable service *type* for the "Add Service" dialog, returned in
/// [`crate::Response::AddableServices`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddableServiceType {
    /// Stable type id (`"redis"`, `"reverb"`, ...).
    pub type_id: String,
    /// Human-facing label.
    pub display_name: String,
    /// `"single"` (at most one instance) or `"per_site"` (one per linked site).
    pub multiplicity: String,
    /// Whether adding an instance requires choosing a linked site.
    pub requires_site: bool,
    /// Whether adding an instance installs a downloadable version.
    pub requires_version: bool,
    /// True for a single-instance type that is already installed - the GUI
    /// disables its picker row.
    pub already_installed: bool,
    /// Installable versions for this platform, ascending (empty for a
    /// version-less type).
    pub available_versions: Vec<String>,
    /// The type's default loopback port.
    pub default_port: u16,
    /// The daemon's suggested next-free port for a new instance (validated again
    /// on submit).
    pub suggested_port: u16,
}

/// What versions of a service are installed vs installable, returned in
/// [`crate::Response::AvailableServices`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAvailability {
    /// Stable service id.
    pub service: String,
    /// Installable versions published for this platform, ascending.
    pub available: Vec<String>,
    /// Versions already installed on disk, ascending.
    pub installed: Vec<String>,
}

/// One `WordPress` core release line, returned in
/// [`crate::Response::WordpressVersions`]. Sourced from the hand-maintained
/// `meta/wordpress-versions.json` in the yerd repo (not wordpress.org - that
/// API only exposes a minimum PHP floor, with no upper bound, which made very
/// old `WordPress` branches look compatible with brand-new PHP releases they
/// were never tested against).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordPressVersionInfo {
    /// The major.minor release line (e.g. `"6.7"`), shown in the GUI.
    pub branch: String,
    /// The newest patch in this branch (e.g. `"6.7.5"`) - what actually gets
    /// passed to `wp core download --version=`. Needed because that command
    /// (and wordpress.org's download URLs) resolve a bare branch like `"6.7"`
    /// to its original, unpatched release, not its latest patch.
    pub latest: String,
    /// Lowest PHP version this branch is compatible with.
    pub min_php: PhpVersion,
    /// Highest PHP version this branch has been tested against.
    pub max_php: PhpVersion,
}

/// One user database in a SQL service, returned in [`crate::Response::Databases`].
///
/// A struct (not a bare `String`) so future fields (size, owner, encoding) can be
/// added additively without a wire break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseSummary {
    /// The database name.
    pub name: String,
}

/// One installable dev tool, returned in [`crate::Response::Tools`].
///
/// A struct (not a bare id) so the GUI can render status without a second lookup.
/// Field order is the wire contract (serde emits in declaration order); keep it
/// stable and additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    /// Stable tool id (`composer`, `node`, `bun`).
    pub id: String,
    /// Human-readable name for the UI.
    pub display_name: String,
    /// Whether the tool is currently installed.
    pub installed: bool,
    /// Installed version (e.g. `2.10.1`, `v24.17.0`), or `None` when not installed.
    pub version: Option<String>,
    /// The commands this tool provides on `PATH` (e.g. `node`, `npm`, `npx`).
    pub binaries: Vec<String>,
    /// `true` when the tool is NOT Yerd-managed but is available on the user's
    /// PATH (e.g. Homebrew / a global Composer install). Mutually exclusive with
    /// `installed`. Skipped on the wire when `false` so the byte shape is stable
    /// for older clients; defaulted on decode for older daemons.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
    /// Where the external tool was found on the user's `PATH` (e.g.
    /// `/opt/homebrew/bin/node`), when `external` is `true`. Not guaranteed to
    /// be absolute - it mirrors whatever `PATH` entry matched, which is
    /// conventionally but not necessarily absolute. `None` when managed or not
    /// installed. `#[serde(default, skip_serializing_if)]` keeps the wire
    /// additive for older clients/daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_path: Option<String>,
}

/// A single doctor finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Machine-readable check identifier.
    pub code: DiagnosisCode,
    /// How serious the finding is.
    pub severity: Severity,
    /// Short human-readable headline.
    pub title: String,
    /// Longer human-readable explanation.
    pub detail: String,
    /// An exact command (or guidance) to resolve it, when applicable.
    pub remedy: Option<String>,
}

/// Severity of a [`Diagnosis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Severity {
    /// Informational / healthy.
    Ok,
    /// A non-fatal problem the user should address.
    Warn,
    /// A problem that breaks expected behaviour.
    Fail,
}

/// Machine-readable identifier for a doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiagnosisCode {
    /// The daemon is not reachable.
    DaemonDown,
    /// A privileged port fell back to its rootless equivalent.
    PortFallback,
    /// The daemon could bind neither the desired nor the fallback web port pair,
    /// so it is serving no sites (degraded). See [`StatusReport::web_unbound`].
    WebPortsUnbound,
    /// A non-Yerd process is listening on a privileged web port (80/443).
    ForeignWebListener,
    /// The daemon could not bind its DNS responder port, so `*.test` names won't
    /// resolve through Yerd until the port is freed or changed. See
    /// [`StatusReport::dns_unbound`].
    DnsPortUnbound,
    /// The local CA is not trusted in the system store.
    CaNotTrusted,
    /// The OS resolver does not route `*.<tld>` to Yerd.
    ResolverNotInstalled,
    /// No PHP versions are installed.
    NoPhpInstalled,
    /// The configured default PHP version is not installed.
    DefaultPhpNotInstalled,
    /// A supervised FPM pool has failed.
    FpmPoolFailed,
    /// A newer PHP patch is available for an installed version.
    PhpUpdateAvailable,
    /// A supervised service (database / cache) has failed.
    ServiceFailed,
    /// No sites are configured.
    NoSites,
    /// Installing the OS resolver replaced a pre-existing file, which Yerd backed up.
    ResolverBackupSaved,
    /// A dev tool is installed but Yerd's `{data}/bin` isn't on the user's PATH,
    /// so the tool's commands won't resolve in the shell (remedy: `yerd path install`).
    BinDirNotOnPath,
    /// The bundled PHP does not trust the Yerd CA: the managed `{data}/cacert.pem`
    /// is missing or stale, so PHP HTTPS to `.test` fails (`cURL error 60`).
    PhpCaNotTrusted,
    /// The global symlink-escape protection is turned off, so the proxy will serve
    /// files reached through symlinks that resolve outside a site's own folder.
    SymlinkProtectionDisabled,
    /// Two or more sites claim the same domain. The loser was dropped from
    /// routing when the router was built, and which site wins can depend on the
    /// filesystem scan order of parked directories, so the winner may change
    /// across restarts. See [`StatusReport::shadows`].
    DomainShadowed,
    /// Everything checks out.
    AllGood,
}

/// Result of [`crate::Request::DoctorFix`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixReport {
    /// Fixes the daemon attempted, in order.
    pub performed: Vec<FixResult>,
    /// Remaining findings the user must resolve manually (e.g. privileged ops).
    pub manual: Vec<Diagnosis>,
}

/// Outcome of one attempted auto-fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixResult {
    /// Which check this fix addressed.
    pub code: DiagnosisCode,
    /// Whether the fix succeeded.
    pub ok: bool,
    /// Human-readable detail about what happened.
    pub message: String,
}

/// Which Cloudflare Tunnel tier a published site uses. Wire-level mirror of
/// `yerd_tunnel::TunnelKind` (this crate stays free of that dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TunnelKind {
    /// Ephemeral `*.trycloudflare.com` tunnel (no account).
    Quick,
    /// Named tunnel on the user's Cloudflare domain (stable hostname).
    Named,
}

/// Live run state of a supervised tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TunnelRunState {
    /// The `cloudflared` process is alive and serving.
    Running,
    /// The process has exited unexpectedly.
    Failed,
}

/// One live tunnel, as reported in [`crate::Response::Tunnels`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelInfo {
    /// The site the tunnel publishes.
    pub site: String,
    /// Quick vs Named.
    pub kind: TunnelKind,
    /// Whether the process is alive or has died.
    pub state: TunnelRunState,
    /// The public URL (Quick tunnels) once captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The configured public hostname (Named tunnels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// One named tunnel recorded on the account, for
/// [`crate::Response::NamedTunnels`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedTunnelMeta {
    /// The tunnel name.
    pub name: String,
    /// The tunnel UUID.
    pub uuid: String,
}

/// One site enabled in the named tunnel: its public hostname mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteHostname {
    /// The local site name.
    pub site: String,
    /// The public hostname it is exposed at.
    pub hostname: String,
}

/// Where the `cloudflared` binary Yerd is using came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CloudflaredSource {
    /// Downloaded and verified by Yerd into its own managed install dir.
    Managed,
    /// A pre-existing binary found on the user's `PATH`.
    System,
}

/// `cloudflared` install / account status, reported alongside the live tunnels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflaredStatus {
    /// Whether the `cloudflared` binary is installed.
    pub installed: bool,
    /// The installed `cloudflared` version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where `installed`'s binary came from, when installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CloudflaredSource>,
    /// Whether a Cloudflare account is logged in (a `cert.pem` is present).
    #[serde(default)]
    pub logged_in: bool,
}
