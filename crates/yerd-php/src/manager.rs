//! `PhpManager` — drives the pure state machine through real I/O.
//!
//! The manager holds one `Pool<S::Child>` per supervised PHP version. Each
//! pool tracks its current [`PoolState`], the wall-clock baseline used to
//! compute [`Elapsed`], the rendered [`PoolConfig`], and the live child
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
//! `ensure` removes any leftover Unix socket file under the planned path
//! before spawning (ignoring `ENOENT`), and `stop` removes it on the way
//! out. These are the only two serialisation points against stale
//! sockets; if you add a third, document it here.

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
use crate::pool::PoolConfig;
use crate::pure::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, STOP_GRACE,
};
use crate::pure::{env_scrub, fpm_conf};
use crate::traits::{ChildHandle, Clock, HealthProbe, ProcessSpawner};

/// Number of `AllocatedListen::plan` attempts when the kernel-assigned
/// TCP port is briefly claimed by another process. On Unix this is a
/// no-op (no binding happens), so the planner runs at most once.
const MAX_BIND_ATTEMPTS: usize = 5;
/// Per-attempt `FastCGI` probe timeout.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Floor between probe attempts — prevents hot-spin when the listener
/// briefly returns connection-refused.
const HEALTH_PROBE_GAP: Duration = Duration::from_millis(100);

/// Where the pool is in its lifecycle.
struct Pool<Ch: ChildHandle> {
    state: PoolState,
    state_since: Instant,
    cfg: PoolConfig,
    child: Option<Ch>,
}

/// Live run state of a supervised pool, as reported by
/// [`PhpManager::snapshots`].
///
/// The manager only ever *stores* pools that were healthy at insert time, so a
/// snapshot is either `Running` (the master process is still alive) or `Failed`
/// (the master has since exited). "No pool at all" — installed but never started
/// — is not represented here; the daemon fills that in as `Stopped`.
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
    ini_settings: Vec<(String, String)>,
    instance_id: u32,
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
            ini_settings: Vec::new(),
            instance_id,
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
    /// Stored as `(name, value)` pairs and injected into each pool's rendered
    /// FPM config on the next `ensure` (a running pool keeps its current config
    /// until restarted — the daemon restarts live pools after calling this).
    pub fn set_ini_settings(&mut self, settings: Vec<(String, String)>) {
        self.ini_settings = settings;
    }

    /// Ensure FPM is running for `v` and return its listen address.
    ///
    /// Idempotent: if the pool is already `Running` and the child is
    /// still alive, returns the cached listen address immediately. Else
    /// plans an address, renders the config, spawns FPM, and waits for
    /// a healthy probe before returning.
    #[allow(clippy::too_many_lines)] // linear: plan → prepare dirs → render → drive
    pub async fn ensure(&mut self, v: PhpVersion) -> Result<Listen, PhpError> {
        let binary = self
            .binaries
            .get(&v)
            .cloned()
            .ok_or(PhpError::VersionNotInstalled { version: v })?;

        // Fast path: already Running and child still alive.
        if let Some(pool) = self.pools.get_mut(&v) {
            if matches!(pool.state, PoolState::Running { .. }) {
                let still_alive = match pool.child.as_mut() {
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
                if still_alive {
                    return Ok(pool.cfg.listen.clone());
                }
            }
        }

        // Plan listen address with retry (Windows port-pair race).
        let mut last_err: Option<PhpError> = None;
        let mut planned: Option<AllocatedListen> = None;
        for _ in 0..MAX_BIND_ATTEMPTS {
            match AllocatedListen::plan(v, &self.dirs, self.instance_id, &self.binder) {
                Ok(p) => {
                    planned = Some(p);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let listen = planned
            .ok_or_else(|| {
                last_err.unwrap_or(PhpError::Bind {
                    source: yerd_platform::PlatformError::Unsupported {
                        operation: "AllocatedListen::plan",
                    },
                })
            })?
            .listen;

        // Clean any stale Unix socket file.
        if let Listen::UnixSocket(ref path) = listen {
            let _ = fs::remove_file(path);
        }

        // Build config + render + write. Inject the current global ini settings
        // so a (re)started pool picks up the latest values.
        let mut cfg = PoolConfig::dev_defaults(v, listen, &self.dirs, self.instance_id);
        cfg.ini = self.ini_settings.clone();

        // FPM does not create parent directories for its config, pid file, or
        // error_log — and the per-user state dir may not exist yet on first run.
        // Create them up front so FPM initialisation does not fail with ENOENT.
        for path in [&cfg.config_path, &cfg.pid_file, &cfg.error_log] {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| PhpError::ConfigWrite {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let rendered = fpm_conf::render_fpm_conf(&cfg);
        atomic_write::write(&cfg.config_path, rendered.as_bytes()).map_err(|source| {
            PhpError::ConfigWrite {
                path: cfg.config_path.clone(),
                source,
            }
        })?;

        // Snapshot + scrub env once; the builder closure clones into each Command.
        let env = env_scrub::allowlist(&std::env::vars().collect::<Vec<_>>());
        let cmd_builder = || build_cmd(&binary, &cfg.config_path, &env);

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
                self.pools.insert(
                    v,
                    Pool {
                        state: PoolState::Running { pid },
                        state_since: result.state_since,
                        cfg,
                        child: Some(child),
                    },
                );
                Ok(listen)
            }
            Outcome::Stopped => {
                // Stopped from EnsureRequested can't happen via the supervisor
                // table — defensive default.
                Err(PhpError::Spawn {
                    version: v,
                    reason: SpawnFailureReason::Other,
                    source: io::Error::other("ensure: drive returned Stopped"),
                })
            }
        }
    }

    /// Restart the pool: stop it cleanly, then `ensure` again.
    pub async fn restart(&mut self, v: PhpVersion) -> Result<Listen, PhpError> {
        // Surface stop errors but continue so the next ensure() gets a
        // chance — pool is gone from `self.pools` either way.
        let _ = self.stop(v).await;
        self.ensure(v).await
    }

    /// Stop the pool for `v`. No-op if there is no pool.
    pub async fn stop(&mut self, v: PhpVersion) -> Result<(), PhpError> {
        let Some(mut pool) = self.pools.remove(&v) else {
            return Ok(());
        };

        let child = pool.child.take();
        let cfg = pool.cfg.clone();
        let result = self
            .drive(
                v,
                pool.state,
                pool.state_since,
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

    /// Report a live snapshot of every supervised pool.
    ///
    /// Read-only intent, but takes `&mut self` because liveness uses
    /// [`ChildHandle::try_wait`] (which needs `&mut` on the handle). A pool whose
    /// child has exited — or whose stored state is somehow non-`Running` — is
    /// reported as [`PoolRunState::Failed`]; an alive child is `Running` with its
    /// PID. This does **not** reconcile the pool set (no insert/remove); the next
    /// `ensure`/`restart` does that.
    pub fn snapshots(&mut self) -> Vec<PoolSnapshot> {
        let mut out = Vec::with_capacity(self.pools.len());
        for (version, pool) in &mut self.pools {
            let listen = Some(pool.cfg.listen.clone());
            let (state, pid) = match (&pool.state, pool.child.as_mut()) {
                (PoolState::Running { pid }, Some(child)) => match child.try_wait() {
                    Ok(None) => (PoolRunState::Running, Some(*pid)),
                    // Exited (Ok(Some)) or unreadable (Err) ⇒ the master is gone.
                    _ => (PoolRunState::Failed, None),
                },
                // Stored as non-Running, or Running with no child handle: failed.
                _ => (PoolRunState::Failed, None),
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
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
            let (next, action) = transition(state, pending);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => match state {
                    PoolState::Running { pid } => {
                        // `child` is the local from the most recent Spawn.
                        let ch = child.take().ok_or_else(|| PhpError::Spawn {
                            version: v,
                            reason: SpawnFailureReason::Other,
                            source: io::Error::other("drive: Running with no child handle"),
                        })?;
                        return Ok(DriveResult {
                            outcome: Outcome::Running { child: ch, pid },
                            state_since,
                        });
                    }
                    PoolState::Stopped => {
                        return Ok(DriveResult {
                            outcome: Outcome::Stopped,
                            state_since,
                        });
                    }
                    other => {
                        // Driver invariant violation — see module docs.
                        #[allow(clippy::panic)]
                        {
                            panic!(
                                "supervisor: Action::None in non-terminal state {other:?}; \
                                 driver invariant violated"
                            );
                        }
                    }
                },

                Action::Spawn => {
                    let builder = cmd_builder.ok_or_else(|| PhpError::Spawn {
                        version: v,
                        reason: SpawnFailureReason::Other,
                        source: io::Error::other(
                            "drive: Spawn without cmd_builder (entry point bug)",
                        ),
                    })?;
                    let cmd = builder();
                    match self.spawner.spawn(cmd) {
                        Ok(ch) => {
                            let pid = ch.id();
                            child = Some(ch);
                            pending = Event::SpawnSucceeded { pid };
                        }
                        Err(source) => {
                            return Err(PhpError::Spawn {
                                version: v,
                                reason: SpawnFailureReason::from_kind(source.kind()),
                                source,
                            });
                        }
                    }
                }

                Action::HealthCheck => {
                    // Cadence floor: skip the gap on the very first probe
                    // of a Starting window (when elapsed is essentially 0)
                    // but sleep on every subsequent retry so connection-refused
                    // failures don't hot-spin.
                    let elapsed_now = self.clock.now().saturating_duration_since(state_since);
                    if elapsed_now > Duration::from_millis(0) {
                        tokio::time::sleep(HEALTH_PROBE_GAP).await;
                    }

                    let ch = child.as_mut().ok_or_else(|| PhpError::Spawn {
                        version: v,
                        reason: SpawnFailureReason::Other,
                        source: io::Error::other("HealthCheck with no child handle"),
                    })?;

                    let probe_fut =
                        tokio::time::timeout(HEALTH_PROBE_TIMEOUT, self.probe.probe(&cfg.listen));
                    let probe_outcome;
                    let wait_outcome;
                    tokio::select! {
                        probe = probe_fut => { probe_outcome = Some(probe); wait_outcome = None; }
                        exit = ch.wait() => { probe_outcome = None; wait_outcome = Some(exit); }
                    }

                    if let Some(p) = probe_outcome {
                        if matches!(p, Ok(Ok(()))) {
                            pending = Event::HealthCheckOk;
                        } else {
                            // Compute elapsed AFTER the probe finished, so
                            // probe duration counts against the window.
                            let elapsed =
                                Elapsed(self.clock.now().saturating_duration_since(state_since));
                            pending = Event::HealthCheckTick {
                                elapsed_since_starting: elapsed,
                            };
                        }
                    } else if let Some(exit) = wait_outcome {
                        let reason = exit.map_err(|source| PhpError::Spawn {
                            version: v,
                            reason: SpawnFailureReason::WaitFailed,
                            source,
                        })?;
                        child = None;
                        pending = Event::Crashed { reason };
                    }
                }

                Action::Backoff { wait } => {
                    tokio::time::sleep(wait).await;
                    pending = Event::BackoffElapsed;
                }

                Action::Kill { signal } => {
                    if let Some(ch) = child.as_mut() {
                        ch.kill(signal)
                            .await
                            .map_err(|source| PhpError::Kill { version: v, source })?;
                    }
                    pending = wait_after_kill(&mut child, state, signal, v).await?;
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
}

/// Post-kill follow-up: wait for the child to exit (with or without a grace
/// budget) and return the synthetic event the supervisor expects next.
async fn wait_after_kill<Ch: ChildHandle>(
    child: &mut Option<Ch>,
    state: PoolState,
    signal: KillSignal,
    v: PhpVersion,
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
                () = tokio::time::sleep(STOP_GRACE) => {
                    // Child still alive — put it back so the next iteration
                    // can SIGKILL it.
                    *child = Some(owned);
                    return Ok(Event::StopTick {
                        elapsed_since_stopping: Elapsed(STOP_GRACE),
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

fn build_cmd(binary: &PathBuf, config_path: &PathBuf, env: &[(String, String)]) -> StdCommand {
    let mut cmd = StdCommand::new(binary);
    cmd.arg("--fpm-config").arg(config_path);
    cmd.env_clear();
    for (k, val) in env {
        cmd.env(k, val);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
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
