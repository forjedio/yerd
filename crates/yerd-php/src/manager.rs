//! `PhpManager` - drives the pure state machine through real I/O.
//!
//! The manager holds one `Pool<S::Child>` per supervised PHP version, and each
//! pool holds [`WORKERS_PER_VERSION`] lazily spawned `Worker<S::Child>`s plus
//! the round-robin cursor [`PhpManager::ensure`] hands their addresses out
//! with. Each worker tracks its current [`PoolState`], the wall-clock baseline
//! used to compute [`Elapsed`], the rendered [`PoolConfig`], and the live child
//! (when one exists).
//!
//! ## Driver invariants
//!
//! Inside [`PhpManager::drive`], the events fed into [`transition`] never
//! produce `Action::None` in a non-terminal state. Specifically:
//!
//! - The driver never feeds `Event::EnsureRequested` mid-loop. The
//!   *initial* event is supplied by `ensure`/`stop`; subsequent events
//!   come from completed actions only.
//! - The driver never feeds `Event::StopTick` after a SIGKILL has been
//!   sent (the SIGKILL branch waits unconditionally and feeds
//!   `Event::StopComplete`).
//!
//! Any future refactor that breaks these invariants must replace the
//! `panic!` on the `Action::None` non-terminal arm with a real fallback.
//!
//! ## Unix socket cleanup
//!
//! `ensure_worker_once` removes any leftover Unix socket file under the
//! planned path before spawning (ignoring `ENOENT`), and `stop_worker`
//! removes it on the way out. These are the only two serialisation points
//! against stale sockets; if you add a third, document it here.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use yerd_core::PhpVersion;
use yerd_platform::{ActivePortBinder, PlatformDirs};

use crate::error::{ExitReason, PhpError, SpawnFailureReason};
use crate::io::atomic_write;
use crate::listen::{AllocatedListen, Listen};
use crate::pool::{ExtLoad, PoolConfig};
use crate::pure::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
use crate::pure::{env_scrub, fpm_conf};
use crate::traits::{ChildHandle, Clock, HealthProbe, ProcessSpawner};

/// Number of `AllocatedListen::plan` attempts when the kernel-assigned
/// TCP port is briefly claimed by another process. On Unix this is a
/// no-op (no binding happens), so the planner runs at most once.
const MAX_BIND_ATTEMPTS: usize = 5;
/// On-disk dump-extension file name for the host OS. Must match what
/// `bin/yerdd`'s `ext_install` writes (`yerd-dump.dll` on Windows, `.so`
/// elsewhere); a filename-pinning test on each side keeps them in lockstep, as
/// the two live in different crates and can't share a constant.
#[cfg(windows)]
const DUMP_EXT_FILE: &str = "yerd-dump.dll";
#[cfg(not(windows))]
const DUMP_EXT_FILE: &str = "yerd-dump.so";
/// Per-attempt `FastCGI` probe timeout.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Floor between probe attempts - prevents hot-spin when the listener
/// briefly returns connection-refused.
const HEALTH_PROBE_GAP: Duration = Duration::from_millis(100);

/// How many server processes are supervised per PHP version.
///
/// Windows has no FPM SAPI: a pool is a `php-cgi.exe`, a single-request NTS
/// `FastCGI` server with no pre-fork, so one process serves exactly one request
/// at a time. Four of them cover a page plus its sub-requests and a second tab,
/// at roughly 4x the idle RSS of a single interpreter. Everywhere else FPM
/// pre-forks its own workers and [`AllocatedListen::plan`] hands out one socket
/// path per version, so a second worker would only collide on it: the count
/// stays 1 and a pool degenerates to a single supervised child.
///
/// Fixed at compile time on purpose. There is deliberately no `yerd.toml`, IPC
/// or GUI surface for pool sizing; changing it is a one-line edit here.
pub const WORKERS_PER_VERSION: usize = if cfg!(windows) { 4 } else { 1 };

/// One supervised server process inside a version's pool.
struct Worker<Ch: ChildHandle> {
    state: PoolState,
    state_since: Instant,
    cfg: PoolConfig,
    child: Option<Ch>,
}

impl<Ch: ChildHandle> Worker<Ch> {
    /// This worker's PID while it is `Running` with a still-live child.
    /// A `try_wait` error counts as not alive, which is what a status report
    /// should show for a handle it can no longer interrogate.
    fn live_pid(&mut self) -> Option<u32> {
        let PoolState::Running { pid } = self.state else {
            return None;
        };
        let child = self.child.as_mut()?;
        matches!(child.try_wait(), Ok(None)).then_some(pid)
    }
}

/// Every worker supervised for one PHP version, plus the round-robin cursor.
///
/// A slot stays `None` until the worker at that index has come up healthy, and
/// the pool itself only exists because at least one of them did: see
/// [`PhpManager::store_worker`].
struct Pool<Ch: ChildHandle> {
    workers: Vec<Option<Worker<Ch>>>,
    cursor: usize,
}

/// Live run state of a supervised pool, as reported by
/// [`PhpManager::snapshots`].
///
/// The manager only ever *stores* a pool once one of its workers came up
/// healthy, so a snapshot is either `Running` (at least one worker is still
/// alive) or `Failed` (every stored worker has since exited). The "no pool at
/// all" case, installed but never started, is deliberately not represented
/// here; the daemon fills that in as `Stopped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolRunState {
    /// The FPM master process is alive.
    Running,
    /// The FPM master process has exited unexpectedly.
    Failed,
}

/// A point-in-time view of one supervised pool, for status reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSnapshot {
    /// The PHP version this pool serves.
    pub version: PhpVersion,
    /// Whether the master is alive or has died.
    pub state: PoolRunState,
    /// The FPM master PID, when running.
    pub pid: Option<u32>,
    /// The address FPM is configured to listen on.
    pub listen: Option<Listen>,
}

/// What [`PhpManager::drive`] returns on success.
struct DriveResult<Ch: ChildHandle> {
    outcome: Outcome<Ch>,
    state_since: Instant,
}

enum Outcome<Ch: ChildHandle> {
    Running { child: Ch, pid: u32 },
    Stopped,
}

/// Daemon-managed dump-extension loading config (see [`PhpManager::set_dump_ext`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpExtSettings {
    /// Base dir holding per-version extensions: `so_dir/php-<ver>/yerd-dump.so`.
    pub so_dir: PathBuf,
    /// Extra `-d key=value` defines applied when the extension loads (e.g. the
    /// extension's `yerd_dump.state_path`).
    pub ini_defines: Vec<(String, String)>,
}

/// Top-level PHP-FPM pool manager.
///
/// Holds one supervised pool per PHP version. Spawns FPM, health-checks it,
/// restarts on crash, and tears down cleanly on shutdown.
pub struct PhpManager<S, C, P>
where
    S: ProcessSpawner,
    C: Clock,
    P: HealthProbe,
{
    spawner: S,
    clock: C,
    probe: P,
    dirs: PlatformDirs,
    binder: ActivePortBinder,
    pools: BTreeMap<PhpVersion, Pool<S::Child>>,
    binaries: BTreeMap<PhpVersion, PathBuf>,
    ini_settings: BTreeMap<String, String>,
    ini_overrides: BTreeMap<PhpVersion, BTreeMap<String, String>>,
    directives: BTreeMap<PhpVersion, BTreeMap<String, String>>,
    pool_overrides: BTreeMap<PhpVersion, BTreeMap<String, String>>,
    dump_ext: Option<DumpExtSettings>,
    extensions: BTreeMap<PhpVersion, Vec<ExtLoad>>,
    ca_bundle: Option<PathBuf>,
    instance_id: u32,
    /// Timing/restart policy fed to the pure state machine. FPM pools use the
    /// fast-start / cheap-retry profile.
    policy: SupervisorPolicy,
}

impl<S, C, P> PhpManager<S, C, P>
where
    S: ProcessSpawner,
    C: Clock,
    P: HealthProbe,
{
    /// Construct a new manager.
    ///
    /// `binaries` is the map of bundled PHP installs, built by the daemon
    /// during startup. `instance_id` is the daemon's `std::process::id()`;
    /// it disambiguates Unix socket paths across concurrent Yerd
    /// instances on the same host.
    pub fn new(
        spawner: S,
        clock: C,
        probe: P,
        dirs: PlatformDirs,
        binder: ActivePortBinder,
        instance_id: u32,
        binaries: BTreeMap<PhpVersion, PathBuf>,
    ) -> Self {
        Self {
            spawner,
            clock,
            probe,
            dirs,
            binder,
            pools: BTreeMap::new(),
            binaries,
            ini_settings: BTreeMap::new(),
            ini_overrides: BTreeMap::new(),
            directives: BTreeMap::new(),
            pool_overrides: BTreeMap::new(),
            dump_ext: None,
            extensions: BTreeMap::new(),
            ca_bundle: None,
            instance_id,
            policy: SupervisorPolicy::fpm(),
        }
    }

    /// Replace the set of known PHP binaries.
    ///
    /// The map is otherwise a startup snapshot, so a PHP version installed at
    /// runtime (`yerd install php`) is invisible to a long-running manager until
    /// this is called. The daemon refreshes it after a successful install so the
    /// next `ensure` can find the new binary. Existing running pools are
    /// untouched; only future lookups change.
    pub fn set_binaries(&mut self, binaries: BTreeMap<PhpVersion, PathBuf>) {
        self.binaries = binaries;
    }

    /// Replace the global PHP ini settings applied to every pool.
    ///
    /// Injected into each pool's rendered FPM config on the next `ensure` (a
    /// running pool keeps its current config until restarted - the daemon
    /// restarts live pools after calling this).
    pub fn set_ini_settings(&mut self, settings: BTreeMap<String, String>) {
        self.ini_settings = settings;
    }

    /// Replace the sparse per-version overrides of the global ini settings,
    /// keyed by PHP version. Merged over [`Self::set_ini_settings`]'s map via
    /// [`yerd_core::php_settings::merge_effective`] when a pool's config is
    /// rendered. Takes effect on the next `ensure` / restart of a pool, like
    /// `set_extensions`.
    pub fn set_ini_overrides(&mut self, overrides: BTreeMap<PhpVersion, BTreeMap<String, String>>) {
        self.ini_overrides = overrides;
    }

    /// Replace the free-form per-version ini directives applied to each pool,
    /// keyed by PHP version. Rendered as `php_value[...]` lines after the
    /// typed settings. Takes effect on the next `ensure` / restart of a pool,
    /// like `set_extensions`.
    pub fn set_directives(&mut self, directives: BTreeMap<PhpVersion, BTreeMap<String, String>>) {
        self.directives = directives;
    }

    /// Replace the per-version FPM pool settings, keyed by PHP version. These
    /// are pool-block values rather than ini directives, so they never reach a
    /// CLI `php.ini`; a version with no valid override keeps
    /// [`PoolConfig::dev_defaults`]'s value. Takes effect on the next `ensure`
    /// / restart of a pool, like `set_directives`.
    pub fn set_pool_overrides(
        &mut self,
        overrides: BTreeMap<PhpVersion, BTreeMap<String, String>>,
    ) {
        self.pool_overrides = overrides;
    }

    /// Configure daemon-managed dump-extension loading. When set, each pool that
    /// has a matching `yerd-dump.so` under `so_dir/php-<ver>/` (re)starts with
    /// `-d zend_extension=<so>` plus the provided `-d key=value` defines (e.g.
    /// the extension's state-file path). Takes effect on the next `ensure` /
    /// restart of a pool. `None` disables extension loading.
    pub fn set_dump_ext(&mut self, settings: Option<DumpExtSettings>) {
        self.dump_ext = settings;
    }

    /// Replace the user-registered custom extensions applied to each pool, keyed
    /// by PHP version. Each pool loads its version's entries via
    /// `-d [zend_]extension=<path>` on the next `ensure` / restart (a running
    /// pool keeps its current config until restarted). A `.so` missing on disk at
    /// spawn time is skipped with a warning, so a stale entry never blocks start.
    pub fn set_extensions(&mut self, extensions: BTreeMap<PhpVersion, Vec<ExtLoad>>) {
        self.extensions = extensions;
    }

    /// The user extensions to load for `v`, dropping any whose `.so` is missing
    /// on disk (a stale path from a Homebrew ABI-dir bump) with a warning, so a
    /// vanished file never blocks pool start.
    fn resolve_user_extensions(&self, v: PhpVersion) -> Vec<ExtLoad> {
        let Some(exts) = self.extensions.get(&v) else {
            return Vec::new();
        };
        exts.iter()
            .filter(|e| {
                let present = e.path.is_file();
                if !present {
                    tracing::warn!(
                        version = %v,
                        path = %e.path.display(),
                        "skipping registered PHP extension: file not found"
                    );
                }
                present
            })
            .cloned()
            .collect()
    }

    /// Set the managed CA bundle every pool points PHP at (`openssl.cafile` /
    /// `curl.cainfo`), so PHP trusts the Yerd CA on `.test` HTTPS. `None`
    /// leaves PHP's compiled-in default untouched. Takes effect on the next
    /// `ensure` / restart of a pool.
    pub fn set_ca_bundle(&mut self, path: Option<PathBuf>) {
        self.ca_bundle = path;
    }

    /// Ensure a PHP server for `v` is running and return the address the caller
    /// should send this request to.
    ///
    /// Every version is served by [`WORKERS_PER_VERSION`] workers on their own
    /// addresses, handed out round-robin: each call advances the pool's cursor
    /// and returns the worker it lands on. That worker is spawned lazily, so a
    /// version that only ever serves one request at a time never starts more
    /// than one process. Landing on a worker that is already `Running` with a
    /// live child returns its cached address immediately; otherwise the worker
    /// is planned, rendered, spawned and health-checked before returning.
    pub async fn ensure(&mut self, v: PhpVersion) -> Result<Listen, PhpError> {
        let binary = self
            .binaries
            .get(&v)
            .cloned()
            .ok_or(PhpError::VersionNotInstalled { version: v })?;

        let index = self.next_worker_index(v);
        if let Some(listen) = self.running_listen(v, index)? {
            return Ok(listen);
        }
        self.ensure_worker(v, index, &binary).await
    }

    /// The worker index this `ensure` serves, advancing the pool's round-robin
    /// cursor past it. A version with no pool yet always starts at worker 0;
    /// its cursor is seeded when that first worker is stored.
    fn next_worker_index(&mut self, v: PhpVersion) -> usize {
        let Some(pool) = self.pools.get_mut(&v) else {
            return 0;
        };
        let index = pool.cursor % WORKERS_PER_VERSION;
        pool.cursor = pool.cursor.wrapping_add(1);
        index
    }

    /// Bring worker `i` of `v` up, replanning its address and retrying once on
    /// Windows.
    ///
    /// [`AllocatedListen::plan`] binds `127.0.0.1:0`, reads the port and drops
    /// the listener before php-cgi ever sees it, so another process can claim
    /// that port in the gap. Once it has, the address baked into the worker's
    /// config is dead and every restart attempt inside [`Self::drive`] burns on
    /// it, so a second [`Self::ensure_worker_once`] with a freshly planned
    /// address is the only thing that recovers. Unix plans a socket path rather
    /// than a port and has no such window, so it never retries.
    ///
    /// The retry means one `ensure` can drive the state machine twice, i.e. up
    /// to `2 * policy.max_restart_attempts` spawns.
    /// `ensure_surfaces_permanent_failure` in `tests/supervisor_states.rs`
    /// provisions exactly two drives' worth of spawn plans, which is what keeps
    /// it ending in `PermanentFailure` rather than in `PhpError::Spawn` from an
    /// exhausted fake queue. Trimming that plan count, or raising
    /// `max_restart_attempts`, would make this retry surface the wrong error.
    async fn ensure_worker(
        &mut self,
        v: PhpVersion,
        i: usize,
        binary: &std::path::Path,
    ) -> Result<Listen, PhpError> {
        #[cfg(not(windows))]
        {
            self.ensure_worker_once(v, i, binary).await
        }
        #[cfg(windows)]
        {
            let first = self.ensure_worker_once(v, i, binary).await;
            if !matches!(
                first,
                Err(PhpError::HealthCheckTimedOut { .. } | PhpError::PermanentFailure { .. })
            ) {
                return first;
            }
            tracing::warn!(
                version = %v,
                worker = i,
                "php-cgi worker did not come up; replanning its port and retrying once"
            );
            self.drop_worker(v, i);
            self.ensure_worker_once(v, i, binary).await
        }
    }

    /// Forget the worker stored at `i`, so a retry starts from an empty slot.
    #[cfg(windows)]
    fn drop_worker(&mut self, v: PhpVersion, i: usize) {
        if let Some(slot) = self.pools.get_mut(&v).and_then(|p| p.workers.get_mut(i)) {
            *slot = None;
        }
    }

    /// One start attempt for worker `i` of `v`: plan an address, render the
    /// config, spawn, and health-check. Stores the worker only once it is
    /// `Running`.
    #[allow(clippy::too_many_lines)]
    async fn ensure_worker_once(
        &mut self,
        v: PhpVersion,
        i: usize,
        binary: &std::path::Path,
    ) -> Result<Listen, PhpError> {
        let listen = self.plan_listen(v)?;

        if let Listen::UnixSocket(ref path) = listen {
            let _ = fs::remove_file(path);
        }

        let mut cfg = PoolConfig::dev_defaults(v, listen, &self.dirs, self.instance_id);
        let no_overrides = BTreeMap::new();
        let overrides = self.ini_overrides.get(&v).unwrap_or(&no_overrides);
        cfg.ini = yerd_core::php_settings::merge_effective(&self.ini_settings, overrides)
            .into_iter()
            .collect();
        cfg.directives = self
            .directives
            .get(&v)
            .map(|m| m.iter().map(|(k, val)| (k.clone(), val.clone())).collect())
            .unwrap_or_default();
        if let Some(n) = yerd_core::php_pool::override_max_children(self.pool_overrides.get(&v)) {
            cfg.max_children = n;
        }
        cfg.ca_bundle = self.ca_bundle.clone();

        if let Some(ext) = &self.dump_ext {
            let so = ext.so_dir.join(format!("php-{v}")).join(DUMP_EXT_FILE);
            if so.is_file() {
                cfg.extension = Some(so);
                cfg.ini_defines = ext.ini_defines.clone();
            }
        }

        cfg.user_extensions = self.resolve_user_extensions(v);

        for path in [&cfg.config_path, &cfg.pid_file, &cfg.error_log] {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| PhpError::ConfigWrite {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        // Unix renders the FPM pool config; Windows has no FPM, so it renders the
        // supplemental php-cgi ini (loaded via `PHP_INI_SCAN_DIR`) instead.
        #[cfg(not(windows))]
        let rendered = fpm_conf::render_fpm_conf(&cfg);
        #[cfg(windows)]
        let rendered = fpm_conf::render_win_ini(&cfg);
        atomic_write::write(&cfg.config_path, rendered.as_bytes()).map_err(|source| {
            PhpError::ConfigWrite {
                path: cfg.config_path.clone(),
                source,
            }
        })?;

        let env = env_scrub::allowlist(&std::env::vars().collect::<Vec<_>>());
        let extension = cfg.extension.clone();
        let ini_defines = cfg.ini_defines.clone();
        let user_extensions = cfg.user_extensions.clone();
        let listen = cfg.listen.clone();
        #[cfg(windows)]
        let error_log = cfg.error_log.clone();
        let cmd_builder = || {
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut cmd = build_cmd(
                binary,
                &cfg.config_path,
                &listen,
                &env,
                extension.as_deref(),
                &ini_defines,
                &user_extensions,
            );
            #[cfg(windows)]
            attach_child_log(&mut cmd, &error_log);
            cmd
        };

        let initial_state = PoolState::Stopped;
        let initial_since = self.clock.now();
        let result = self
            .drive(
                v,
                initial_state,
                initial_since,
                None,
                Event::EnsureRequested,
                &cfg,
                Some(&cmd_builder),
            )
            .await?;

        match result.outcome {
            Outcome::Running { child, pid } => {
                let listen = cfg.listen.clone();
                self.store_worker(
                    v,
                    i,
                    Worker {
                        state: PoolState::Running { pid },
                        state_since: result.state_since,
                        cfg,
                        child: Some(child),
                    },
                );
                Ok(listen)
            }
            Outcome::Stopped => Err(PhpError::Spawn {
                version: v,
                reason: SpawnFailureReason::Other,
                source: io::Error::other("ensure: drive returned Stopped"),
            }),
        }
    }

    /// Record a healthy worker at index `i`, creating the version's pool if this
    /// is its first live worker.
    ///
    /// The pool is created here and nowhere else. A version that has never
    /// started must stay absent from `pools`, because the daemon renders "no
    /// pool" as `Stopped` while an empty pool would read as
    /// [`PoolRunState::Failed`] and start the snapshot-driven restart loops
    /// against a version that was never asked to run.
    fn store_worker(&mut self, v: PhpVersion, i: usize, worker: Worker<S::Child>) {
        let pool = self.pools.entry(v).or_insert_with(|| Pool {
            workers: std::iter::repeat_with(|| None)
                .take(WORKERS_PER_VERSION)
                .collect(),
            cursor: i.wrapping_add(1),
        });
        if let Some(slot) = pool.workers.get_mut(i) {
            *slot = Some(worker);
        }
    }

    /// Fast path for [`Self::ensure`]: if worker `i` of pool `v` is `Running`
    /// with a still-live child, return its cached listen address; else `None`.
    fn running_listen(&mut self, v: PhpVersion, i: usize) -> Result<Option<Listen>, PhpError> {
        let Some(Some(worker)) = self.pools.get_mut(&v).and_then(|p| p.workers.get_mut(i)) else {
            return Ok(None);
        };
        if !matches!(worker.state, PoolState::Running { .. }) {
            return Ok(None);
        }
        let still_alive = match worker.child.as_mut() {
            Some(ch) => ch
                .try_wait()
                .map_err(|source| PhpError::Spawn {
                    version: v,
                    reason: SpawnFailureReason::WaitFailed,
                    source,
                })?
                .is_none(),
            None => false,
        };
        Ok(still_alive.then(|| worker.cfg.listen.clone()))
    }

    /// Plan a listen address, retrying up to `MAX_BIND_ATTEMPTS` to absorb the
    /// Windows port-pair race. On Windows a short, per-daemon-varying backoff
    /// between attempts de-synchronises two planners colliding on the same
    /// ephemeral port instead of retrying hot.
    fn plan_listen(&self, v: PhpVersion) -> Result<Listen, PhpError> {
        let mut last_err: Option<PhpError> = None;
        for attempt in 0..MAX_BIND_ATTEMPTS {
            match AllocatedListen::plan(v, &self.dirs, self.instance_id, &self.binder) {
                Ok(p) => return Ok(p.listen),
                Err(e) => last_err = Some(e),
            }
            #[cfg(windows)]
            if attempt + 1 < MAX_BIND_ATTEMPTS {
                let jitter =
                    10 + (u64::from(self.instance_id).wrapping_mul(attempt as u64 + 1) % 40);
                std::thread::sleep(Duration::from_millis(jitter));
            }
            #[cfg(not(windows))]
            let _ = attempt;
        }
        Err(last_err.unwrap_or(PhpError::Bind {
            source: yerd_platform::PlatformError::Unsupported {
                operation: "AllocatedListen::plan",
            },
        }))
    }

    /// Restart the pool: stop it cleanly, then `ensure` again.
    ///
    /// Every worker is stopped. The `ensure` brings the first one back, and the
    /// rest are respawned lazily as later calls rotate onto their indices.
    pub async fn restart(&mut self, v: PhpVersion) -> Result<Listen, PhpError> {
        let _ = self.stop(v).await;
        self.ensure(v).await
    }

    /// Stop every worker of the pool for `v`. No-op if there is no pool.
    ///
    /// The pool is dropped whatever happens, and the first worker error (if
    /// any) is returned once they have all been driven down.
    pub async fn stop(&mut self, v: PhpVersion) -> Result<(), PhpError> {
        let Some(pool) = self.pools.remove(&v) else {
            return Ok(());
        };

        let mut first_err: Option<PhpError> = None;
        for worker in pool.workers.into_iter().flatten() {
            if let Err(e) = self.stop_worker(v, worker).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Drive one worker down to `Stopped` and remove its Unix socket.
    async fn stop_worker(
        &mut self,
        v: PhpVersion,
        mut worker: Worker<S::Child>,
    ) -> Result<(), PhpError> {
        let child = worker.child.take();
        let cfg = worker.cfg;
        let result = self
            .drive(
                v,
                worker.state,
                worker.state_since,
                child,
                Event::StopRequested,
                &cfg,
                None,
            )
            .await;

        if let Listen::UnixSocket(ref path) = cfg.listen {
            let _ = fs::remove_file(path);
        }

        result.map(|_| ())
    }

    /// Stop every supervised pool in deterministic order.
    pub async fn shutdown(&mut self) -> Result<(), PhpError> {
        let versions: Vec<PhpVersion> = self.pools.keys().copied().collect();
        let mut first_err: Option<PhpError> = None;
        for v in versions {
            if let Err(e) = self.stop(v).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Report a live snapshot of every supervised pool, aggregated to **one row
    /// per PHP version** rather than one per worker.
    ///
    /// Read-only intent, but takes `&mut self` because liveness uses
    /// [`ChildHandle::try_wait`] (which needs `&mut` on the handle). A pool with
    /// at least one live worker is [`PoolRunState::Running`], carrying that
    /// worker's PID and address; a pool whose stored workers have all exited (or
    /// whose stored state is somehow non-`Running`) is
    /// [`PoolRunState::Failed`] with no PID and the first stored worker's
    /// address. Reporting `Running` while any worker still serves matches what
    /// the user observes, and a dead worker is repaired by the next `ensure`
    /// that lands on its index. This does **not** reconcile the pool set (no
    /// insert/remove); the next `ensure`/`restart` does that.
    pub fn snapshots(&mut self) -> Vec<PoolSnapshot> {
        let mut out = Vec::with_capacity(self.pools.len());
        for (version, pool) in &mut self.pools {
            let mut first_listen: Option<Listen> = None;
            let mut live: Option<(u32, Listen)> = None;
            for worker in pool.workers.iter_mut().flatten() {
                if first_listen.is_none() {
                    first_listen = Some(worker.cfg.listen.clone());
                }
                if live.is_some() {
                    continue;
                }
                if let Some(pid) = worker.live_pid() {
                    live = Some((pid, worker.cfg.listen.clone()));
                }
            }
            let (state, pid, listen) = match live {
                Some((pid, listen)) => (PoolRunState::Running, Some(pid), Some(listen)),
                None => (PoolRunState::Failed, None, first_listen),
            };
            out.push(PoolSnapshot {
                version: *version,
                state,
                pid,
                listen,
            });
        }
        out
    }

    /// Pump the pure state machine to a terminal state, doing the I/O
    /// each `Action` requires.
    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &mut self,
        v: PhpVersion,
        mut state: PoolState,
        mut state_since: Instant,
        mut child: Option<S::Child>,
        initial: Event,
        cfg: &PoolConfig,
        cmd_builder: Option<&(dyn Fn() -> StdCommand + Sync)>,
    ) -> Result<DriveResult<S::Child>, PhpError> {
        let mut pending = initial;
        loop {
            let (next, action) = transition(state, pending, &self.policy);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => {
                    return Self::finish_terminal(state, &mut child, v, state_since);
                }

                Action::Spawn => {
                    pending = self.spawn_child(v, cmd_builder, &mut child)?;
                }

                Action::HealthCheck => {
                    pending = self.health_check(v, cfg, state_since, &mut child).await?;
                }

                Action::Backoff { wait } => {
                    tokio::time::sleep(wait).await;
                    pending = Event::BackoffElapsed;
                }

                Action::Kill { signal } => {
                    if let Some(ch) = child.as_mut() {
                        ch.kill(signal, StopProtocol::GroupTerm)
                            .await
                            .map_err(|source| PhpError::Kill { version: v, source })?;
                    }
                    pending = wait_after_kill(&mut child, state, signal, v, self.policy.stop_grace)
                        .await?;
                }

                Action::EmitError(ErrorTag::HealthCheckTimedOut) => {
                    return Err(PhpError::HealthCheckTimedOut {
                        version: v,
                        attempts: starting_attempts(state),
                    });
                }
                Action::EmitError(ErrorTag::PermanentFailure) => {
                    let (reason, _) = failed_reason(state);
                    return Err(PhpError::PermanentFailure { version: v, reason });
                }
            }
        }
    }

    /// Handle `Action::None`: a terminal state yields a [`DriveResult`]; any
    /// other state is a driver-invariant violation (see module docs) and panics.
    fn finish_terminal(
        state: PoolState,
        child: &mut Option<S::Child>,
        v: PhpVersion,
        state_since: Instant,
    ) -> Result<DriveResult<S::Child>, PhpError> {
        match state {
            PoolState::Running { pid } => {
                let ch = child.take().ok_or_else(|| PhpError::Spawn {
                    version: v,
                    reason: SpawnFailureReason::Other,
                    source: io::Error::other("drive: Running with no child handle"),
                })?;
                Ok(DriveResult {
                    outcome: Outcome::Running { child: ch, pid },
                    state_since,
                })
            }
            PoolState::Stopped => Ok(DriveResult {
                outcome: Outcome::Stopped,
                state_since,
            }),
            other => {
                #[allow(clippy::panic)]
                {
                    panic!(
                        "supervisor: Action::None in non-terminal state {other:?}; \
                         driver invariant violated"
                    );
                }
            }
        }
    }

    /// Handle `Action::Spawn`: build + spawn the command, record the child, and
    /// return the follow-up event.
    fn spawn_child(
        &mut self,
        v: PhpVersion,
        cmd_builder: Option<&(dyn Fn() -> StdCommand + Sync)>,
        child: &mut Option<S::Child>,
    ) -> Result<Event, PhpError> {
        let builder = cmd_builder.ok_or_else(|| PhpError::Spawn {
            version: v,
            reason: SpawnFailureReason::Other,
            source: io::Error::other("drive: Spawn without cmd_builder (entry point bug)"),
        })?;
        let cmd = builder();
        match self.spawner.spawn(cmd) {
            Ok(ch) => {
                let pid = ch.id();
                *child = Some(ch);
                Ok(Event::SpawnSucceeded { pid })
            }
            Err(source) => Err(PhpError::Spawn {
                version: v,
                reason: SpawnFailureReason::from_kind(source.kind()),
                source,
            }),
        }
    }

    /// Handle `Action::HealthCheck`: probe readiness, racing the child's exit,
    /// and return the follow-up event. The cadence floor skips the gap on the
    /// first probe of a `Starting` window but sleeps on every retry so
    /// connection-refused failures don't hot-spin.
    async fn health_check(
        &mut self,
        v: PhpVersion,
        cfg: &PoolConfig,
        state_since: Instant,
        child: &mut Option<S::Child>,
    ) -> Result<Event, PhpError> {
        let elapsed_now = self.clock.now().saturating_duration_since(state_since);
        if elapsed_now > Duration::from_millis(0) {
            tokio::time::sleep(HEALTH_PROBE_GAP).await;
        }

        let ch = child.as_mut().ok_or_else(|| PhpError::Spawn {
            version: v,
            reason: SpawnFailureReason::Other,
            source: io::Error::other("HealthCheck with no child handle"),
        })?;

        let probe_fut = tokio::time::timeout(HEALTH_PROBE_TIMEOUT, self.probe.probe(&cfg.listen));
        let probe_outcome;
        let wait_outcome;
        tokio::select! {
            probe = probe_fut => { probe_outcome = Some(probe); wait_outcome = None; }
            exit = ch.wait() => { probe_outcome = None; wait_outcome = Some(exit); }
        }

        if let Some(p) = probe_outcome {
            if matches!(p, Ok(Ok(()))) {
                Ok(Event::HealthCheckOk)
            } else {
                let elapsed = Elapsed(self.clock.now().saturating_duration_since(state_since));
                Ok(Event::HealthCheckTick {
                    elapsed_since_starting: elapsed,
                })
            }
        } else if let Some(exit) = wait_outcome {
            let reason = exit.map_err(|source| PhpError::Spawn {
                version: v,
                reason: SpawnFailureReason::WaitFailed,
                source,
            })?;
            *child = None;
            Ok(Event::Crashed { reason })
        } else {
            Err(PhpError::Spawn {
                version: v,
                reason: SpawnFailureReason::Other,
                source: io::Error::other("HealthCheck: select resolved neither arm"),
            })
        }
    }
}

/// Post-kill follow-up: wait for the child to exit (with or without a grace
/// budget) and return the synthetic event the supervisor expects next.
async fn wait_after_kill<Ch: ChildHandle>(
    child: &mut Option<Ch>,
    state: PoolState,
    signal: KillSignal,
    v: PhpVersion,
    stop_grace: std::time::Duration,
) -> Result<Event, PhpError> {
    match (state, signal) {
        (PoolState::Stopping { sigkilled: false }, KillSignal::Term) => {
            let Some(mut owned) = child.take() else {
                return Ok(Event::StopComplete);
            };
            let event = tokio::select! {
                exit = owned.wait() => {
                    exit.map_err(|source| PhpError::Spawn {
                        version: v,
                        reason: SpawnFailureReason::WaitFailed,
                        source,
                    })?;
                    Event::StopComplete
                }
                () = tokio::time::sleep(stop_grace) => {
                    *child = Some(owned);
                    return Ok(Event::StopTick {
                        elapsed_since_stopping: Elapsed(stop_grace),
                    });
                }
            };
            Ok(event)
        }
        (PoolState::Stopping { sigkilled: true }, _) => {
            if let Some(ch) = child.as_mut() {
                ch.wait().await.map_err(|source| PhpError::Spawn {
                    version: v,
                    reason: SpawnFailureReason::WaitFailed,
                    source,
                })?;
            }
            *child = None;
            Ok(Event::StopComplete)
        }
        (PoolState::Starting { .. }, KillSignal::Term) => {
            if let Some(ch) = child.as_mut() {
                ch.wait().await.map_err(|source| PhpError::Spawn {
                    version: v,
                    reason: SpawnFailureReason::WaitFailed,
                    source,
                })?;
            }
            *child = None;
            Ok(Event::Crashed {
                reason: ExitReason::Unknown,
            })
        }
        _ => Ok(Event::StopComplete),
    }
}

/// Point php-cgi's stdio at the pool's instance log.
///
/// Unix needs no equivalent: FPM opens `error_log` itself, from the pool config
/// [`fpm_conf::render_fpm_conf`] renders. Windows has no FPM, so without this
/// the child's stdio is inherited from a daemon that usually has no console and
/// PHP's startup diagnostics - a `.dll` that won't load, a malformed ini - are
/// discarded, leaving a crash loop with no recorded cause.
///
/// **Both streams are captured, and stdout is the one that matters.** Verified
/// against the bundled 8.5 build: a bad `-d extension=` writes its
/// "Unable to load dynamic library" warning to *stdout*, not stderr, and
/// `display_errors=stderr` does not move it. Capturing stdout is safe precisely
/// because this is `FastCGI` mode: responses travel over the `-b` socket, so
/// nothing but these diagnostics is ever written there. stderr is captured too,
/// since a hard failure below the PHP layer can still land on it.
///
/// Best-effort by design: a log that cannot be opened must not stop the pool
/// from starting, so the failure is warned about and the child runs without
/// capture. Appends, so restarts accumulate rather than truncating the evidence.
#[cfg(windows)]
fn attach_child_log(cmd: &mut StdCommand, error_log: &std::path::Path) {
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(error_log)
    {
        Ok(file) => file,
        Err(source) => {
            tracing::warn!(
                path = %error_log.display(),
                error = %source,
                "could not open the pool log; php-cgi output will not be captured"
            );
            return;
        }
    };
    match file.try_clone() {
        Ok(dup) => {
            cmd.stdout(std::process::Stdio::from(file));
            cmd.stderr(std::process::Stdio::from(dup));
        }
        Err(source) => {
            tracing::warn!(
                path = %error_log.display(),
                error = %source,
                "could not duplicate the pool log handle; capturing php-cgi stdout only"
            );
            cmd.stdout(std::process::Stdio::from(file));
        }
    }
}

/// Build the pool's server command.
///
/// Unix runs `php-fpm --fpm-config <conf>` in its own process group. Windows has
/// no FPM SAPI: it runs `php-cgi.exe -b <host:port>` (the `FastCGI` server),
/// pointing `PHP_INI_SCAN_DIR` at the supplemental ini's directory (`config_path`
/// is that ini file). `-c` is deliberately **not** passed on Windows: it would
/// drop the bundle's own `php.ini` (which carries `extension_dir`, the enabled
/// extension set, and the install-time CA lines). `PHP_FCGI_MAX_REQUESTS=0`
/// stops php-cgi from exiting after N requests (the supervisor would count that
/// as a crash), and `PHP_FCGI_CHILDREN` is cleared (fork-based, unsupported on
/// Windows).
#[allow(clippy::too_many_lines)]
fn build_cmd(
    binary: &std::path::Path,
    config_path: &std::path::Path,
    listen: &Listen,
    env: &[(String, String)],
    extension: Option<&std::path::Path>,
    ini_defines: &[(String, String)],
    user_extensions: &[ExtLoad],
) -> StdCommand {
    let mut cmd = StdCommand::new(binary);
    if let Some(so) = extension {
        cmd.arg("-d").arg(format!("extension={}", so.display()));
        for (k, val) in ini_defines {
            cmd.arg("-d").arg(format!("{k}={val}"));
        }
    }
    for ext in user_extensions {
        let directive = if ext.zend {
            "zend_extension"
        } else {
            "extension"
        };
        cmd.arg("-d")
            .arg(format!("{directive}={}", ext.path.display()));
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;

        let _ = listen;
        cmd.arg("--fpm-config").arg(config_path);
        cmd.env_clear();
        for (k, val) in env {
            cmd.env(k, val);
        }
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        let addr = match listen {
            Listen::TcpLoopback(a) => a.to_string(),
            Listen::UnixSocket(_) => format!("{}:0", std::net::Ipv4Addr::LOCALHOST),
        };
        cmd.arg("-b").arg(addr);
        cmd.env_clear();
        for (k, val) in env {
            cmd.env(k, val);
        }
        cmd.env("PHP_FCGI_MAX_REQUESTS", "0");
        cmd.env_remove("PHP_FCGI_CHILDREN");
        if let Some(scan_dir) = config_path.parent() {
            cmd.env("PHP_INI_SCAN_DIR", scan_dir);
        }
    }
    cmd
}

fn starting_attempts(s: PoolState) -> u32 {
    match s {
        PoolState::Starting { attempts, .. } => attempts,
        _ => 0,
    }
}

fn failed_reason(s: PoolState) -> (ExitReason, u32) {
    match s {
        PoolState::Failed {
            last_exit,
            attempts,
        } => (last_exit, attempts),
        _ => (ExitReason::Unknown, 0),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod pure_helper_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    fn args_of(cmd: &StdCommand) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// The trailing args after the shared `-d ...` block: `--fpm-config <conf>`
    /// on Unix, `-b <addr>` on Windows (php-cgi has no `--fpm-config`).
    fn tail(config: &str, addr: &str) -> Vec<String> {
        #[cfg(not(windows))]
        {
            let _ = addr;
            vec!["--fpm-config".to_owned(), config.to_owned()]
        }
        #[cfg(windows)]
        {
            let _ = config;
            vec!["-b".to_owned(), addr.to_owned()]
        }
    }

    fn tcp_listen() -> Listen {
        Listen::TcpLoopback("127.0.0.1:9000".parse().unwrap())
    }

    #[test]
    fn build_cmd_without_extension_passes_no_defines() {
        let binary = PathBuf::from("/opt/php/bin/php");
        let config = PathBuf::from("/run/yerd/fpm-8.3.conf");
        let env = vec![("PATH".to_owned(), "/usr/bin".to_owned())];
        let cmd = build_cmd(&binary, &config, &tcp_listen(), &env, None, &[], &[]);

        assert_eq!(cmd.get_program(), OsStr::new("/opt/php/bin/php"));
        let args = args_of(&cmd);
        assert_eq!(args, tail("/run/yerd/fpm-8.3.conf", "127.0.0.1:9000"));
        assert!(!args.iter().any(|a| a == "-d"));

        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("PATH".to_owned(), Some("/usr/bin".to_owned()))));
        #[cfg(windows)]
        {
            assert!(
                envs.contains(&("PHP_FCGI_MAX_REQUESTS".to_owned(), Some("0".to_owned()))),
                "{envs:?}"
            );
            assert!(
                envs.iter().any(|(k, _)| k == "PHP_INI_SCAN_DIR"),
                "{envs:?}"
            );
        }
    }

    #[test]
    fn build_cmd_with_extension_emits_defines_first() {
        let binary = PathBuf::from("/opt/php/bin/php");
        let config = PathBuf::from("/run/yerd/fpm-8.3.conf");
        let env: Vec<(String, String)> = vec![];
        let so = Path::new("/lib/yerd-dump.so");
        let defines = vec![("yerd_dump.state_path".to_owned(), "/var/state".to_owned())];
        let cmd = build_cmd(
            &binary,
            &config,
            &tcp_listen(),
            &env,
            Some(so),
            &defines,
            &[],
        );

        let args = args_of(&cmd);
        let mut want = vec![
            "-d".to_owned(),
            "extension=/lib/yerd-dump.so".to_owned(),
            "-d".to_owned(),
            "yerd_dump.state_path=/var/state".to_owned(),
        ];
        want.extend(tail("/run/yerd/fpm-8.3.conf", "127.0.0.1:9000"));
        assert_eq!(args, want);
        let ext_pos = args
            .iter()
            .position(|a| a.starts_with("extension="))
            .unwrap();
        assert!(ext_pos < args.len() - 1, "defines come before the tail");
    }

    #[test]
    fn build_cmd_emits_user_extensions_with_and_without_dump_ext() {
        let binary = PathBuf::from("/opt/php/bin/php");
        let config = PathBuf::from("/run/yerd/fpm-8.5.conf");
        let env: Vec<(String, String)> = vec![];
        let user = vec![
            ExtLoad {
                path: PathBuf::from("/lib/scrypt.so"),
                zend: false,
            },
            ExtLoad {
                path: PathBuf::from("/lib/xdebug.so"),
                zend: true,
            },
        ];
        let cmd = build_cmd(&binary, &config, &tcp_listen(), &env, None, &[], &user);
        let args = args_of(&cmd);
        let mut want = vec![
            "-d".to_owned(),
            "extension=/lib/scrypt.so".to_owned(),
            "-d".to_owned(),
            "zend_extension=/lib/xdebug.so".to_owned(),
        ];
        want.extend(tail("/run/yerd/fpm-8.5.conf", "127.0.0.1:9000"));
        assert_eq!(args, want);

        let so = Path::new("/lib/yerd-dump.so");
        let cmd = build_cmd(&binary, &config, &tcp_listen(), &env, Some(so), &[], &user);
        let args = args_of(&cmd);
        let mut want = vec![
            "-d".to_owned(),
            "extension=/lib/yerd-dump.so".to_owned(),
            "-d".to_owned(),
            "extension=/lib/scrypt.so".to_owned(),
            "-d".to_owned(),
            "zend_extension=/lib/xdebug.so".to_owned(),
        ];
        want.extend(tail("/run/yerd/fpm-8.5.conf", "127.0.0.1:9000"));
        assert_eq!(args, want);
    }

    #[test]
    fn starting_attempts_reads_attempts_from_starting_else_zero() {
        assert_eq!(
            starting_attempts(PoolState::Starting {
                attempts: 4,
                pid: Some(9),
            }),
            4
        );
        assert_eq!(starting_attempts(PoolState::Running { pid: 1 }), 0);
        assert_eq!(starting_attempts(PoolState::Stopped), 0);
    }

    #[test]
    fn failed_reason_extracts_exit_and_attempts_else_default() {
        let (reason, attempts) = failed_reason(PoolState::Failed {
            last_exit: ExitReason::Code(7),
            attempts: 3,
        });
        assert_eq!(reason, ExitReason::Code(7));
        assert_eq!(attempts, 3);

        let (reason, attempts) = failed_reason(PoolState::Running { pid: 1 });
        assert_eq!(reason, ExitReason::Unknown);
        assert_eq!(attempts, 0);
    }
}
