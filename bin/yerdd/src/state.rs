//! Shared daemon runtime state.
//!
//! Holds the authoritative config (behind a mutex that serializes mutations)
//! and the live routing table (a [`yerd_proxy::SharedRouter`] the proxy reads
//! from). The IPC mutation path takes the config mutex, applies a change,
//! validates and persists it, then swaps the router under its write guard.
//! Lock order is **config-mutex → router-write**, only ever in that path; the
//! proxy and `ListSites` take a router *read* guard and never touch the config
//! mutex, so there is no cross-lock cycle.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{watch, Mutex, Notify, RwLock};

use yerd_core::PhpVersion;
use yerd_ipc::PortStatus;
use yerd_platform::{CaFingerprint, PlatformDirs};
use yerd_proxy::SharedRouter;

use crate::backend_resolver::DaemonPhpManager;
use crate::detect_cache::DetectCache;

/// Mail-capture runtime fact captured at startup: whether the SMTP listener
/// actually bound. `Status` sources `enabled`/`port` from the live config (so a
/// `SetMailPort`/`SetMailEnabled` save is reflected immediately), but whether the
/// server is *actually* bound is a startup property - a config change only takes
/// effect on the next restart - so it lives here.
#[derive(Debug, Clone, Copy)]
pub struct MailRuntime {
    /// Whether the SMTP listener actually bound (and is accepting mail).
    pub listening: bool,
}

/// A minted one-time remote-setup bootstrap code, held in memory only (a daemon
/// restart clears it - fail-closed). See `crate::lan_setup` and the
/// `MintRemoteSetupCode` IPC handler.
pub struct RemoteSetupCode {
    /// The URL-safe code value (compared in constant time by the endpoint).
    pub value: String,
    /// When the code expires.
    pub expires_at: Instant,
    /// Whether the terminal (script) fetch has consumed it (single-use), or a
    /// lockout invalidated it.
    pub used: bool,
    /// Failed match attempts against this code; a lockout invalidates it past a
    /// threshold so a private-IP peer can't brute-force within the TTL.
    pub attempts: u32,
}

/// Everything the IPC dispatch and proxy share at runtime.
pub struct DaemonState {
    /// Authoritative on-disk config, mirrored in memory. The mutex serializes
    /// concurrent mutations so two clients can't race a save.
    pub config: Mutex<yerd_config::Config>,
    /// Live routing table, shared with the proxy. Replaced wholesale on a
    /// successful mutation.
    pub router: SharedRouter,
    /// Resolved per-user directories (used to re-scan parked roots on a
    /// mutation).
    pub dirs: PlatformDirs,
    /// Path the config is loaded from and saved to.
    pub config_path: PathBuf,
    /// Address the embedded DNS responder is bound on (reported by `DaemonInfo`
    /// so `yerd elevate resolver` can route `.test` here).
    pub dns_addr: SocketAddr,
    /// Absolute path to the local CA certificate PEM (reported by `DaemonInfo`).
    pub ca_path: PathBuf,
    /// SHA-256 fingerprint of the CA cert (reported by `DaemonInfo`).
    pub ca_fingerprint: CaFingerprint,
    /// Path to the managed PHP CA bundle (`{data}/cacert.pem` = host public
    /// roots + the Yerd CA) that the bundled PHP verifies TLS against, or
    /// `None` when no host roots were found (PHP keeps its compiled default).
    /// Fed to FPM pools (`set_ca_bundle`) and the CLI `php.ini`.
    pub php_ca_bundle: Option<PathBuf>,
    /// Update cache: installed minor → newest build `(patch, revision)` known
    /// from the last manifest poll. Populated by the periodic checker /
    /// `CheckPhpUpdates` and served (no network) on `ListPhp`.
    pub php_updates: RwLock<HashMap<PhpVersion, (String, u32)>>,
    /// Yerd self-update cache: the releases seen at the last GitHub poll. Empty
    /// until the first successful fetch. Populated by the periodic checker /
    /// `CheckUpdate` and served (no network) when a live fetch fails.
    pub yerd_update: RwLock<Vec<yerd_update::ReleaseMeta>>,
    /// The last persisted self-update result (loaded from `{state}/update-check.json`
    /// at boot, refreshed on every successful poll / `CheckUpdate`). Served by
    /// `CachedUpdateStatus` so the UI can pre-fill the Updates section on load and
    /// show a "last checked …" time without a network round-trip.
    pub update_snapshot: RwLock<Option<crate::self_update::UpdateSnapshot>>,
    /// The FPM pool supervisor, shared with the proxy backend resolver and the
    /// update task. `yerd status` / `yerd doctor` read live pool state from it.
    pub php_manager: Arc<Mutex<DaemonPhpManager>>,
    /// The database/cache service supervisor (Redis/Valkey in Phase 1). Holds
    /// one supervised instance per engine; status/doctor read live state from it.
    pub service_manager: Arc<Mutex<crate::services::DaemonServiceManager>>,
    /// The Cloudflare Tunnel supervisor. Holds one supervised `cloudflared`
    /// child per shared site; `TunnelStatus` reads live state from it. Quick
    /// tunnels are ephemeral (not persisted) and torn down on daemon shutdown.
    pub tunnel_manager: Arc<Mutex<crate::tunnel::DaemonTunnelManager>>,
    /// Cache of which `cloudflared` binary to use (Yerd-managed or a
    /// `PATH`-found system install) and its version, so the `--version` probe
    /// of a system binary runs once rather than on every tunnel action or
    /// status poll. See `tunnel::resolved_cloudflared`.
    pub cloudflared_resolution: RwLock<Option<crate::tunnel::install::Resolved>>,
    /// Captured-mail store (the built-in SMTP sink writes here; IPC reads/clears
    /// it). Always present even when capture is disabled, so stored mail remains
    /// listable/clearable after the server is turned off.
    pub mail_store: Arc<yerd_mail::Store>,
    /// Mail-capture runtime facts, surfaced in `Status`. `listening` reflects
    /// whether the SMTP port was actually bound (it can be `enabled && !listening`
    /// when the port was busy at startup - a non-fatal condition).
    pub mail: MailRuntime,
    /// HTTP listener: requested vs actually-bound port (reported by `Status`).
    pub http: PortStatus,
    /// HTTPS listener: requested vs actually-bound port (reported by `Status`).
    pub https: PortStatus,
    /// Port the HTTP→HTTPS redirect `Location` header currently advertises.
    /// Starts at `https.bound`; when `https.fell_back`, a background prober
    /// (`redirect_probe_handle` in `lib.rs`) flips it to `https.requested`
    /// once it observes a live privileged-port redirect (macOS `pf`, via
    /// `yerd elevate ports`) via `yerd_platform::PortRedirector::is_active`,
    /// and back when that redirect goes away - so the proxy can advertise a
    /// portless `https://site.test` without restarting. Shared with
    /// `yerd_proxy::HttpsBinding::public_port`.
    pub redirect_https_port: Arc<AtomicU16>,
    /// Live mirror of `config.symlink_protection`, shared with `yerd_proxy`'s
    /// static-file/script-resolution path so the `SetSymlinkProtection` IPC
    /// handler can toggle the proxy's symlink-escape guard without a daemon
    /// restart. The config mutex remains the durable source of truth; this
    /// atomic is the hot read the proxy consults per request.
    pub symlink_protection: Arc<AtomicBool>,
    /// Set when the daemon could bind neither the desired nor the fallback web
    /// ports - it runs degraded (no proxy). Carries the fallback ports it failed
    /// on, surfaced in `Status` (`web_unbound`) so the UI/doctor can name them.
    pub web_unbound: Option<yerd_ipc::UnboundWeb>,
    /// Set when the daemon could not bind its DNS responder port - it runs
    /// degraded (no name resolution). Carries the configured `dns_port` it failed
    /// on, surfaced in `Status` (`dns_unbound`) so the UI/doctor can name it.
    pub dns_unbound: Option<u16>,
    /// Per-process id (see `StatusReport::boot_id`) clients use to detect a
    /// completed restart across the pid-preserving re-exec.
    pub boot_id: u64,
    /// When the daemon finished bringing up (for `Status` uptime).
    pub started_at: Instant,
    /// Broadcast shutdown trigger. Owned by state so the `RestartDaemon` IPC
    /// handler can request a graceful teardown (every task watches a clone of
    /// this channel exactly like it does for SIGTERM).
    pub shutdown_tx: watch::Sender<bool>,
    /// Set by `RestartDaemon` before tripping `shutdown_tx`, so the top level
    /// re-execs in place instead of exiting.
    pub restart_requested: AtomicBool,
    /// Web-root detection cache, shared between the mutation path and the
    /// filesystem watcher so repeated parked-root rescans stay cheap.
    pub detect_cache: Arc<DetectCache>,
    /// Pinged after a config mutation commits so the filesystem watcher
    /// reconciles its watch set (e.g. a newly-parked root) without waiting for
    /// an unrelated filesystem event.
    pub watch_dirty: Notify,
    /// Dump-telemetry ring buffer + server control, shared with the dump-server
    /// task and the IPC dump handlers.
    pub dumps: Arc<crate::dump_server::DumpStore>,
    /// Serializes `php_install::reconcile_shims` runs. IPC dispatch is
    /// `tokio::spawn`-per-connection, so two clients can reconcile the `{data}/bin`
    /// shims concurrently; this guard keeps the (sync) scan→prune from interleaving.
    pub shim_reconcile: Mutex<()>,
    /// Serializes dev-tool (`composer`/`node`/`bun`) install/uninstall mutations.
    /// IPC dispatch is `tokio::spawn`-per-connection, so two clients could swap
    /// `{data}/tools/<id>` concurrently; this guard makes commit+reconcile atomic.
    pub tool_mutate: Mutex<()>,
    /// Serializes `cloudflared` install (and Phase-2 login) mutations of
    /// `{data}/tunnel`, so two clients can't clobber the staging binary.
    pub tunnel_mutate: Mutex<()>,
    /// Serializes PHP-version install/update mutations. IPC dispatch is
    /// `tokio::spawn`-per-connection (and the streamed install is its own task),
    /// so two clients could install concurrently; the staging dir is keyed by
    /// version (+ this daemon's pid), so two installs of the *same* version would
    /// otherwise clobber each other's staging + race the final rename.
    pub php_mutate: Mutex<()>,
    /// Serializes PHP ini-settings mutations (`set/unset php`, global and
    /// per-version). The config read-modify-save and the follow-up apply to the
    /// live `PhpManager` release the config lock in between, so without this
    /// guard an overlapping request could replay a stale settings snapshot into
    /// the manager after a newer one was already applied.
    pub php_settings_mutate: Mutex<()>,
    /// Background-job registry. Long-running operations (site creation) run as
    /// jobs whose streamed progress the client polls via `JobStatus`.
    pub jobs: crate::jobs::JobRegistry,
    /// Site names held by an in-flight `CreateSite` job, so two concurrent
    /// creates can't both scaffold (then fight over registering) the same name.
    pub reserved_names: Mutex<HashSet<String>>,
    /// WordPress core-version availability cache (the WordPress wizard's
    /// version dropdown): the last successful `meta/wordpress-versions.json`
    /// fetch, plus when it happened so a request can decide whether to
    /// re-fetch. `None` until the first successful fetch; served (no
    /// network) when still fresh, or as a stale fallback when a re-fetch
    /// fails. See [`crate::wordpress_versions`].
    pub wordpress_versions: RwLock<Option<(Instant, Vec<yerd_ipc::WordPressVersionInfo>)>>,
    /// One-click `WordPress` admin login token store, shared with `yerd-proxy`
    /// via the [`yerd_proxy::LoginTokenConsumer`] trait. See
    /// [`crate::wordpress_login`].
    pub wordpress_login_tokens: Arc<crate::wordpress_login::LoginTokenRegistry>,
    /// Path the `WordPress` auto-login prepend script was written to at
    /// startup (see [`crate::wordpress_login::write_prepend_script`]), or
    /// `None` if writing it failed - one-click login is then unavailable this
    /// boot, but the ordinary, non-authenticated `/wp-admin/` link still
    /// works.
    pub wordpress_login_prepend_script: Option<PathBuf>,
    /// In-memory cache of which sites are `WordPress` (site name → bool),
    /// refreshed on every router rebuild (`startup::build_routing`, run on a
    /// mutation or a filesystem-watcher tick) rather than detected fresh on
    /// every `ListSites` poll - see [`crate::wordpress_detect`].
    pub wordpress_sites: Arc<RwLock<HashMap<String, bool>>>,
    /// In-memory cache of which sites are Laravel (site name → bool), refreshed
    /// on the same router-rebuild hook as `wordpress_sites` rather than detected
    /// on every `ListSites` poll - see [`crate::laravel_detect`].
    pub laravel_sites: Arc<RwLock<HashMap<String, bool>>>,
    /// Whether the LAN remote-setup bootstrap listener actually bound this boot.
    /// Read into `StatusReport::lan_setup_bound` for effective-vs-configured
    /// reporting. Only meaningful when LAN mode is on.
    pub lan_setup_bound: Arc<AtomicBool>,
    /// The current minted one-time remote-setup code, if any (in-memory only, so
    /// a restart invalidates it - fail-closed).
    pub remote_setup_code: Mutex<Option<RemoteSetupCode>>,
}
