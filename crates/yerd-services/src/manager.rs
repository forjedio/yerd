//! `ServiceManager` — drives the shared supervisor state machine for one
//! supervised instance per [`Service`], doing the real I/O.
//!
//! Mirrors `yerd_php::PhpManager` in shape (it drives the same
//! `yerd_supervise` state machine) but differs where databases differ:
//!
//! - **Fixed TCP loopback port** (not an FPM Unix socket), pre-flighted for
//!   conflicts via [`PortBinder`] so a clash surfaces as
//!   [`ServiceError::PortInUse`] rather than a mystery crash loop.
//! - **The database [`SupervisorPolicy`]** (generous readiness window, longer
//!   stop grace) so a slow cold-boot is not killed mid-startup.
//! - A **one-time datadir init** seam before first start (no-op for Redis;
//!   `initdb` / `mysqld --initialize` land in Phase 2).
//!
//! Phase 1 supervises **Redis (Valkey)** only; other engines return
//! [`ServiceError::Unsupported`] until their config/init/probe land in Phase 2.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use yerd_platform::{ActivePortBinder, PlatformDirs, PlatformError, PortBinder};
use yerd_supervise::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, SupervisorPolicy,
};
use yerd_supervise::{
    ChildHandle, Clock, ExitReason, HealthProbe, Listen, ProcessSpawner, SpawnFailureReason,
};

use crate::config_render;
use crate::error::ServiceError;
use crate::service::Service;
use crate::version::{self, ServiceVersion};

/// Per-attempt readiness-probe timeout.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Floor between probe attempts — prevents hot-spin when the listener briefly
/// refuses connections during startup.
const HEALTH_PROBE_GAP: Duration = Duration::from_millis(100);

/// Live run state of a supervised service, as reported by [`ServiceManager::snapshots`].
///
/// "No instance at all" (installed but never started, or stopped) is not
/// represented here — the daemon fills that in as `Stopped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRunState {
    /// The server process is alive.
    Running,
    /// The server process has exited unexpectedly.
    Failed,
}

/// A point-in-time view of one supervised service instance, for status reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSnapshot {
    /// Which service.
    pub service: Service,
    /// The running version.
    pub version: ServiceVersion,
    /// Whether the server is alive or has died.
    pub state: ServiceRunState,
    /// The server PID, when running.
    pub pid: Option<u32>,
    /// The address the server is configured to listen on.
    pub listen: Option<Listen>,
}

/// One supervised service instance.
struct Instance<Ch: ChildHandle> {
    state: PoolState,
    state_since: Instant,
    version: ServiceVersion,
    listen: Listen,
    child: Option<Ch>,
}

/// What [`ServiceManager::drive`] returns on success.
struct DriveResult<Ch: ChildHandle> {
    outcome: Outcome<Ch>,
    state_since: Instant,
}

enum Outcome<Ch: ChildHandle> {
    Running { child: Ch, pid: u32 },
    Stopped,
}

/// Supervises local database / cache services, one instance per [`Service`].
pub struct ServiceManager<S, C, P>
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
    policy: SupervisorPolicy,
    instances: BTreeMap<Service, Instance<S::Child>>,
}

impl<S, C, P> ServiceManager<S, C, P>
where
    S: ProcessSpawner,
    C: Clock,
    P: HealthProbe,
{
    /// Construct a new manager. The database [`SupervisorPolicy`] is applied to
    /// every supervised instance.
    pub fn new(
        spawner: S,
        clock: C,
        probe: P,
        dirs: PlatformDirs,
        binder: ActivePortBinder,
    ) -> Self {
        Self {
            spawner,
            clock,
            probe,
            dirs,
            binder,
            policy: SupervisorPolicy::database(),
            instances: BTreeMap::new(),
        }
    }

    /// Ensure `service` (at `version`, on `port`) is running, returning its
    /// listen address. Idempotent: if already running and alive, returns the
    /// cached address.
    ///
    /// Phase 1 supports Redis only; other engines return
    /// [`ServiceError::Unsupported`].
    pub async fn ensure(
        &mut self,
        service: Service,
        version: ServiceVersion,
        port: u16,
    ) -> Result<Listen, ServiceError> {
        if service != Service::Redis {
            return Err(ServiceError::Unsupported {
                service,
                detail: "only Redis is supported in this build".to_owned(),
            });
        }

        let binary = version::server_path(&self.dirs, service, &version);
        if !binary.is_file() {
            return Err(ServiceError::VersionNotInstalled { service, version });
        }

        // Fast path: already Running and the child is still alive.
        if let Some(inst) = self.instances.get_mut(&service) {
            if matches!(inst.state, PoolState::Running { .. }) {
                let alive = match inst.child.as_mut() {
                    Some(ch) => ch
                        .try_wait()
                        .map_err(|source| ServiceError::Spawn {
                            service,
                            reason: SpawnFailureReason::WaitFailed,
                            source,
                        })?
                        .is_none(),
                    None => false,
                };
                if alive {
                    return Ok(inst.listen.clone());
                }
            }
        }

        // Fixed loopback port; pre-flight for conflicts.
        self.preflight_port(service, port)?;
        let listen = Listen::TcpLoopback(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

        // Datadir + config/log parents.
        let datadir = version::datadir(&self.dirs, service, &version);
        std::fs::create_dir_all(&datadir).map_err(|source| ServiceError::Init {
            service,
            datadir: datadir.clone(),
            detail: source.to_string(),
        })?;
        let config_path = version::config_path(&self.dirs, service);
        let log_path = version::log_path(&self.dirs, service);
        for path in [&config_path, &log_path] {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                    path: parent.to_path_buf(),
                    service,
                    source,
                })?;
            }
        }

        // Render + write the config (Redis only in Phase 1).
        let rendered = config_render::render_redis_conf(port, &datadir, &log_path);
        std::fs::write(&config_path, rendered.as_bytes()).map_err(|source| {
            ServiceError::ConfigWrite {
                path: config_path.clone(),
                service,
                source,
            }
        })?;

        let cmd_builder = || build_cmd(&binary, &config_path);

        let initial_since = self.clock.now();
        let result = self
            .drive(
                service,
                PoolState::Stopped,
                initial_since,
                None,
                Event::EnsureRequested,
                &listen,
                Some(&cmd_builder),
            )
            .await?;

        match result.outcome {
            Outcome::Running { child, pid } => {
                self.instances.insert(
                    service,
                    Instance {
                        state: PoolState::Running { pid },
                        state_since: result.state_since,
                        version,
                        listen: listen.clone(),
                        child: Some(child),
                    },
                );
                Ok(listen)
            }
            Outcome::Stopped => Err(ServiceError::Spawn {
                service,
                reason: SpawnFailureReason::Other,
                source: std::io::Error::other("ensure: drive returned Stopped"),
            }),
        }
    }

    /// Restart the instance: stop it cleanly, then ensure again.
    pub async fn restart(
        &mut self,
        service: Service,
        version: ServiceVersion,
        port: u16,
    ) -> Result<Listen, ServiceError> {
        let _ = self.stop(service).await;
        self.ensure(service, version, port).await
    }

    /// Stop the instance for `service`. No-op if there is none.
    pub async fn stop(&mut self, service: Service) -> Result<(), ServiceError> {
        let Some(mut inst) = self.instances.remove(&service) else {
            return Ok(());
        };
        let child = inst.child.take();
        let listen = inst.listen.clone();
        self.drive(
            service,
            inst.state,
            inst.state_since,
            child,
            Event::StopRequested,
            &listen,
            None,
        )
        .await
        .map(|_| ())
    }

    /// Stop every supervised instance in deterministic order.
    pub async fn shutdown(&mut self) -> Result<(), ServiceError> {
        let services: Vec<Service> = self.instances.keys().copied().collect();
        let mut first_err: Option<ServiceError> = None;
        for service in services {
            if let Err(e) = self.stop(service).await {
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

    /// Report a live snapshot of every supervised instance.
    pub fn snapshots(&mut self) -> Vec<ServiceSnapshot> {
        let mut out = Vec::with_capacity(self.instances.len());
        for (service, inst) in &mut self.instances {
            let listen = Some(inst.listen.clone());
            let (state, pid) = match (&inst.state, inst.child.as_mut()) {
                (PoolState::Running { pid }, Some(child)) => match child.try_wait() {
                    Ok(None) => (ServiceRunState::Running, Some(*pid)),
                    _ => (ServiceRunState::Failed, None),
                },
                _ => (ServiceRunState::Failed, None),
            };
            out.push(ServiceSnapshot {
                service: *service,
                version: inst.version.clone(),
                state,
                pid,
                listen,
            });
        }
        out
    }

    /// Pre-flight a fixed loopback port: bind it, then drop the listener. A
    /// clash surfaces as [`ServiceError::PortInUse`]; any other failure as
    /// [`ServiceError::Bind`]. Best-effort (a TOCTOU window remains before the
    /// server itself binds), but turns the common "something already on 6379"
    /// case into a clear message instead of a crash loop.
    fn preflight_port(&self, service: Service, port: u16) -> Result<(), ServiceError> {
        match self.binder.bind(port) {
            Ok(bound) => {
                drop(bound);
                Ok(())
            }
            Err(PlatformError::Bind { source, .. })
                if source.kind() == std::io::ErrorKind::AddrInUse =>
            {
                Err(ServiceError::PortInUse { service, port })
            }
            Err(source) => Err(ServiceError::Bind {
                service,
                port,
                source,
            }),
        }
    }

    /// Pump the pure state machine to a terminal state, doing the I/O each
    /// `Action` requires. Mirrors `yerd_php::PhpManager::drive`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn drive(
        &mut self,
        service: Service,
        mut state: PoolState,
        mut state_since: Instant,
        mut child: Option<S::Child>,
        initial: Event,
        listen: &Listen,
        cmd_builder: Option<&(dyn Fn() -> StdCommand + Sync)>,
    ) -> Result<DriveResult<S::Child>, ServiceError> {
        let mut pending = initial;
        loop {
            let (next, action) = transition(state, pending, &self.policy);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => match state {
                    PoolState::Running { pid } => {
                        let ch = child.take().ok_or_else(|| ServiceError::Spawn {
                            service,
                            reason: SpawnFailureReason::Other,
                            source: std::io::Error::other("drive: Running with no child handle"),
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
                        // Driver invariant: the driver never feeds an event that
                        // produces `Action::None` in a non-terminal state. This
                        // is the same contract as `PhpManager::drive`.
                        return Err(ServiceError::Spawn {
                            service,
                            reason: SpawnFailureReason::Other,
                            source: std::io::Error::other(format!(
                                "drive: Action::None in non-terminal state {other:?}"
                            )),
                        });
                    }
                },

                Action::Spawn => {
                    let builder = cmd_builder.ok_or_else(|| ServiceError::Spawn {
                        service,
                        reason: SpawnFailureReason::Other,
                        source: std::io::Error::other("drive: Spawn without cmd_builder"),
                    })?;
                    match self.spawner.spawn(builder()) {
                        Ok(ch) => {
                            let pid = ch.id();
                            child = Some(ch);
                            pending = Event::SpawnSucceeded { pid };
                        }
                        Err(source) => {
                            return Err(ServiceError::Spawn {
                                service,
                                reason: SpawnFailureReason::from_kind(source.kind()),
                                source,
                            });
                        }
                    }
                }

                Action::HealthCheck => {
                    let elapsed_now = self.clock.now().saturating_duration_since(state_since);
                    if elapsed_now > Duration::from_millis(0) {
                        tokio::time::sleep(HEALTH_PROBE_GAP).await;
                    }
                    let ch = child.as_mut().ok_or_else(|| ServiceError::Spawn {
                        service,
                        reason: SpawnFailureReason::Other,
                        source: std::io::Error::other("HealthCheck with no child handle"),
                    })?;

                    let probe_fut =
                        tokio::time::timeout(HEALTH_PROBE_TIMEOUT, self.probe.probe(listen));
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
                            let elapsed =
                                Elapsed(self.clock.now().saturating_duration_since(state_since));
                            pending = Event::HealthCheckTick {
                                elapsed_since_starting: elapsed,
                            };
                        }
                    } else if let Some(exit) = wait_outcome {
                        let reason = exit.map_err(|source| ServiceError::Spawn {
                            service,
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
                            .map_err(|source| ServiceError::Kill { service, source })?;
                    }
                    pending =
                        wait_after_kill(&mut child, state, signal, service, self.policy.stop_grace)
                            .await?;
                }

                Action::EmitError(ErrorTag::HealthCheckTimedOut) => {
                    return Err(ServiceError::HealthCheckTimedOut {
                        service,
                        attempts: starting_attempts(state),
                    });
                }
                Action::EmitError(ErrorTag::PermanentFailure) => {
                    return Err(ServiceError::PermanentFailure {
                        service,
                        reason: failed_reason(state),
                    });
                }
            }
        }
    }
}

/// Post-kill follow-up: wait for the child to exit (with or without a grace
/// budget) and return the synthetic event the supervisor expects next. Mirrors
/// `yerd_php`'s helper of the same name.
async fn wait_after_kill<Ch: ChildHandle>(
    child: &mut Option<Ch>,
    state: PoolState,
    signal: KillSignal,
    service: Service,
    stop_grace: Duration,
) -> Result<Event, ServiceError> {
    match (state, signal) {
        (PoolState::Stopping { sigkilled: false }, KillSignal::Term) => {
            let Some(mut owned) = child.take() else {
                return Ok(Event::StopComplete);
            };
            let event = tokio::select! {
                exit = owned.wait() => {
                    exit.map_err(|source| ServiceError::Spawn {
                        service,
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
                ch.wait().await.map_err(|source| ServiceError::Spawn {
                    service,
                    reason: SpawnFailureReason::WaitFailed,
                    source,
                })?;
            }
            *child = None;
            Ok(Event::StopComplete)
        }
        (PoolState::Starting { .. }, KillSignal::Term) => {
            if let Some(ch) = child.as_mut() {
                ch.wait().await.map_err(|source| ServiceError::Spawn {
                    service,
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

/// Build the server command: `<server> <config>`, with its own process group on
/// Unix so the supervisor's `killpg` reaps any children with it.
fn build_cmd(binary: &std::path::Path, config_path: &std::path::Path) -> StdCommand {
    let mut cmd = StdCommand::new(binary);
    cmd.arg(config_path);
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

fn failed_reason(s: PoolState) -> ExitReason {
    match s {
        PoolState::Failed { last_exit, .. } => last_exit,
        _ => ExitReason::Unknown,
    }
}
