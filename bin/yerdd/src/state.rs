//! Shared daemon runtime state.
//!
//! Holds the authoritative config (behind a mutex that serializes mutations)
//! and the live routing table (a [`yerd_proxy::SharedRouter`] the proxy reads
//! from). The IPC mutation path takes the config mutex, applies a change,
//! validates and persists it, then swaps the router under its write guard.
//! Lock order is **config-mutex → router-write**, only ever in that path; the
//! proxy and `ListSites` take a router *read* guard and never touch the config
//! mutex, so there is no cross-lock cycle.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
/// server is *actually* bound is a startup property — a config change only takes
/// effect on the next restart — so it lives here.
#[derive(Debug, Clone, Copy)]
pub struct MailRuntime {
    /// Whether the SMTP listener actually bound (and is accepting mail).
    pub listening: bool,
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
    /// Update cache: installed minor → newest full patch known from the last
    /// distribution poll. Populated by the periodic checker / `CheckPhpUpdates`
    /// and served (no network) on `ListPhp`.
    pub php_updates: RwLock<HashMap<PhpVersion, String>>,
    /// The FPM pool supervisor, shared with the proxy backend resolver and the
    /// update task. `yerd status` / `yerd doctor` read live pool state from it.
    pub php_manager: Arc<Mutex<DaemonPhpManager>>,
    /// The database/cache service supervisor (Redis/Valkey in Phase 1). Holds
    /// one supervised instance per engine; status/doctor read live state from it.
    pub service_manager: Arc<Mutex<crate::services::DaemonServiceManager>>,
    /// Captured-mail store (the built-in SMTP sink writes here; IPC reads/clears
    /// it). Always present even when capture is disabled, so stored mail remains
    /// listable/clearable after the server is turned off.
    pub mail_store: Arc<yerd_mail::Store>,
    /// Mail-capture runtime facts, surfaced in `Status`. `listening` reflects
    /// whether the SMTP port was actually bound (it can be `enabled && !listening`
    /// when the port was busy at startup — a non-fatal condition).
    pub mail: MailRuntime,
    /// HTTP listener: requested vs actually-bound port (reported by `Status`).
    pub http: PortStatus,
    /// HTTPS listener: requested vs actually-bound port (reported by `Status`).
    pub https: PortStatus,
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
}
