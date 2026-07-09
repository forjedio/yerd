//! `ServiceManager` - drives the shared supervisor state machine for one
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
//! - A **one-time datadir init** seam before first start: no-op for Redis;
//!   `initdb` (Postgres) / `mysqld --initialize-insecure` (`MySQL`) /
//!   `mariadb-install-db` (`MariaDB`), run crash-safely into a staging dir.
//!
//! Supervises **Redis (Valkey)**, **`MySQL`**, **`MariaDB`**, and
//! **`PostgreSQL`** - per-engine config rendering, datadir init, and protocol
//! readiness probes are selected from the [`Service`]. (`MariaDB` shares
//! `MySQL`'s supervision path; it differs only in its install/init binaries.)

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use yerd_platform::{ActivePortBinder, PlatformDirs, PlatformError, PortBinder};
use yerd_supervise::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
use yerd_supervise::{ChildHandle, Clock, ExitReason, Listen, ProcessSpawner, SpawnFailureReason};

use crate::config_render;
use crate::error::ServiceError;
use crate::health::ReadinessProbe;
use crate::service::Service;
use crate::version::{self, ServiceVersion};

/// Per-attempt readiness-probe timeout.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Floor between probe attempts - prevents hot-spin when the listener briefly
/// refuses connections during startup.
const HEALTH_PROBE_GAP: Duration = Duration::from_millis(100);

/// Live run state of a supervised service, as reported by [`ServiceManager::snapshots`].
///
/// "No instance at all" (installed but never started, or stopped) is not
/// represented here - the daemon fills that in as `Stopped`.
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
    P: ReadinessProbe,
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
    P: ReadinessProbe,
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
    /// cached address. For an engine that needs it, the datadir is initialised
    /// on first start.
    pub async fn ensure(
        &mut self,
        service: Service,
        version: ServiceVersion,
        port: u16,
    ) -> Result<Listen, ServiceError> {
        let binary = version::server_path(&self.dirs, service, &version);
        if !binary.is_file() {
            return Err(ServiceError::VersionNotInstalled { service, version });
        }

        if let Some(listen) = self.running_listen(service)? {
            return Ok(listen);
        }

        let datadir = version::datadir(&self.dirs, service, &version);
        let config_path = version::config_path(&self.dirs, service);
        let log_path = version::log_path(&self.dirs, service);
        let socket = version::socket_path(&self.dirs, service);

        self.init_datadir_if_needed(service, &version, &datadir, &log_path)
            .await?;

        self.preflight_port(service, port)?;
        let listen = Listen::TcpLoopback(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

        Self::prepare_dirs(service, &datadir, &config_path, &log_path, &socket)?;

        let init_file = version::init_file_path(&self.dirs, service);
        if matches!(service, Service::MySql | Service::MariaDb) {
            std::fs::write(
                &init_file,
                config_render::render_my_bootstrap_sql().as_bytes(),
            )
            .map_err(|source| ServiceError::ConfigWrite {
                path: init_file.clone(),
                service,
                source,
            })?;
        }

        let rendered =
            render_service_config(service, port, &datadir, &socket, &log_path, &init_file);
        std::fs::write(&config_path, rendered.as_bytes()).map_err(|source| {
            ServiceError::ConfigWrite {
                path: config_path.clone(),
                service,
                source,
            }
        })?;

        let cmd_builder = || build_cmd(service, &binary, &config_path, &datadir, &log_path);

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

    /// Fast path for [`Self::ensure`]: if `service` is recorded `Running` and its
    /// child is still alive, return its listen address; otherwise `None`.
    fn running_listen(&mut self, service: Service) -> Result<Option<Listen>, ServiceError> {
        let Some(inst) = self.instances.get_mut(&service) else {
            return Ok(None);
        };
        if !matches!(inst.state, PoolState::Running { .. }) {
            return Ok(None);
        }
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
        Ok(alive.then(|| inst.listen.clone()))
    }

    /// One-time datadir initialisation for the SQL engines (no-op for Redis and
    /// for an already-initialised datadir). MUST run before the
    /// `create_dir_all(datadir)` in [`Self::prepare_dirs`]: `initdb` /
    /// `mysqld --initialize-insecure` populate the datadir themselves (via a
    /// crash-safe staging + rename) and refuse a pre-existing one.
    async fn init_datadir_if_needed(
        &mut self,
        service: Service,
        version: &ServiceVersion,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        if !service.needs_init() {
            return Ok(());
        }
        if is_initialized(datadir, service) {
            if service == Service::Postgres {
                check_pg_major(datadir, version)?;
            }
            return Ok(());
        }
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                path: parent.to_path_buf(),
                service,
                source,
            })?;
        }
        self.init_datadir(service, version, datadir, log_path).await
    }

    /// Create the datadir plus the config/log (and, for MySQL/MariaDB, the Unix
    /// socket) parent directories. The datadir create is idempotent - SQL `init`
    /// already populated it; this is the real creator for Redis (no init).
    fn prepare_dirs(
        service: Service,
        datadir: &std::path::Path,
        config_path: &std::path::Path,
        log_path: &std::path::Path,
        socket: &std::path::Path,
    ) -> Result<(), ServiceError> {
        std::fs::create_dir_all(datadir).map_err(|source| ServiceError::Init {
            service,
            datadir: datadir.to_path_buf(),
            detail: source.to_string(),
        })?;
        let mut parents = vec![config_path, log_path];
        if matches!(service, Service::MySql | Service::MariaDb) {
            parents.push(socket);
        }
        for path in parents {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                    path: parent.to_path_buf(),
                    service,
                    source,
                })?;
            }
        }
        Ok(())
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

    /// One-time datadir initialisation for an engine that needs it. Runs the
    /// engine's init tool into a fresh **staging** dir, then atomically renames
    /// it onto the final datadir - so an interrupted init never leaves a
    /// half-populated datadir behind (only an orphan `.init-staging-*` the next
    /// attempt removes). No-op for an engine with no init binary.
    async fn init_datadir(
        &self,
        service: Service,
        version: &ServiceVersion,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        let Some(init_bin_name) = service.init_binary() else {
            return Ok(());
        };
        let install_dir = version::install_dir(&self.dirs, service, version);
        let init_bin = install_dir.join("bin").join(init_bin_name);
        if !init_bin.is_file() {
            return Err(ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("install is missing bin/{init_bin_name}"),
            });
        }

        let staging = version::service_root(&self.dirs, service)
            .join(format!(".init-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| ServiceError::Init {
            service,
            datadir: datadir.to_path_buf(),
            detail: format!("create staging dir: {e}"),
        })?;

        if let Err(e) = self
            .run_init(service, &init_bin, &install_dir, &staging, datadir, log_path)
            .await
        {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        if datadir.exists() {
            if let Err(e) = std::fs::remove_dir_all(datadir) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ServiceError::Init {
                    service,
                    datadir: datadir.to_path_buf(),
                    detail: format!("remove prior datadir: {e}"),
                });
            }
        }
        if let Some(parent) = datadir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("create datadir parent: {e}"),
            })?;
        }
        std::fs::rename(&staging, datadir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("install datadir: {e}"),
            }
        })
    }

    /// Spawn the engine's init tool one-shot (into `staging`), wait for it, and
    /// require a clean `exit 0`. Init output goes to the service log so a
    /// failure is diagnosable via `yerd service logs`. `datadir` is the FINAL
    /// path, used only for error reporting.
    async fn run_init(
        &self,
        service: Service,
        init_bin: &std::path::Path,
        basedir: &std::path::Path,
        staging: &std::path::Path,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        let args = init_args(service, basedir, staging);
        if args.is_empty() {
            return Ok(());
        }
        let mut cmd = StdCommand::new(init_bin);
        cmd.args(&args);
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            if let Ok(f2) = f.try_clone() {
                cmd.stdout(std::process::Stdio::from(f2));
            }
            cmd.stderr(std::process::Stdio::from(f));
        }

        let mut child = self
            .spawner
            .spawn(cmd)
            .map_err(|source| ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("spawn {}: {source}", init_bin.display()),
            })?;
        let reason = child.wait().await.map_err(|source| ServiceError::Init {
            service,
            datadir: datadir.to_path_buf(),
            detail: format!("wait for init: {source}"),
        })?;
        match reason {
            ExitReason::Code(0) => Ok(()),
            other => Err(ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("init process exited with {other}"),
            }),
        }
    }

    /// Pump the pure state machine to a terminal state, doing the I/O each
    /// `Action` requires. Mirrors `yerd_php::PhpManager::drive`.
    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &mut self,
        service: Service,
        mut state: PoolState,
        mut state_since: Instant,
        mut child: Option<S::Child>,
        initial: Event,
        listen: &Listen,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, ServiceError> + Sync)>,
    ) -> Result<DriveResult<S::Child>, ServiceError> {
        let mut pending = initial;
        loop {
            let (next, action) = transition(state, pending, &self.policy);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => {
                    return Self::finish_terminal(state, &mut child, service, state_since);
                }

                Action::Spawn => {
                    pending = self.spawn_child(service, cmd_builder, &mut child)?;
                }

                Action::HealthCheck => {
                    pending = self
                        .health_check(service, listen, state_since, &mut child)
                        .await?;
                }

                Action::Backoff { wait } => {
                    tokio::time::sleep(wait).await;
                    pending = Event::BackoffElapsed;
                }

                Action::Kill { signal } => {
                    if let Some(ch) = child.as_mut() {
                        ch.kill(signal, stop_protocol(service))
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

    /// Handle `Action::None`: a terminal state yields a [`DriveResult`]; any
    /// other state is a driver-contract violation (the driver never feeds an
    /// event that produces `Action::None` in a non-terminal state).
    fn finish_terminal(
        state: PoolState,
        child: &mut Option<S::Child>,
        service: Service,
        state_since: Instant,
    ) -> Result<DriveResult<S::Child>, ServiceError> {
        match state {
            PoolState::Running { pid } => {
                let ch = child.take().ok_or_else(|| ServiceError::Spawn {
                    service,
                    reason: SpawnFailureReason::Other,
                    source: std::io::Error::other("drive: Running with no child handle"),
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
            other => Err(ServiceError::Spawn {
                service,
                reason: SpawnFailureReason::Other,
                source: std::io::Error::other(format!(
                    "drive: Action::None in non-terminal state {other:?}"
                )),
            }),
        }
    }

    /// Handle `Action::Spawn`: build + spawn the command, record the child, and
    /// return the follow-up event.
    fn spawn_child(
        &mut self,
        service: Service,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, ServiceError> + Sync)>,
        child: &mut Option<S::Child>,
    ) -> Result<Event, ServiceError> {
        let builder = cmd_builder.ok_or_else(|| ServiceError::Spawn {
            service,
            reason: SpawnFailureReason::Other,
            source: std::io::Error::other("drive: Spawn without cmd_builder"),
        })?;
        let cmd = builder()?;
        match self.spawner.spawn(cmd) {
            Ok(ch) => {
                let pid = ch.id();
                *child = Some(ch);
                Ok(Event::SpawnSucceeded { pid })
            }
            Err(source) => Err(ServiceError::Spawn {
                service,
                reason: SpawnFailureReason::from_kind(source.kind()),
                source,
            }),
        }
    }

    /// Handle `Action::HealthCheck`: probe readiness, racing the child's exit,
    /// and return the follow-up event.
    async fn health_check(
        &mut self,
        service: Service,
        listen: &Listen,
        state_since: Instant,
        child: &mut Option<S::Child>,
    ) -> Result<Event, ServiceError> {
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
            tokio::time::timeout(HEALTH_PROBE_TIMEOUT, self.probe.probe(service, listen));
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
            let reason = exit.map_err(|source| ServiceError::Spawn {
                service,
                reason: SpawnFailureReason::WaitFailed,
                source,
            })?;
            *child = None;
            Ok(Event::Crashed { reason })
        } else {
            Err(ServiceError::Spawn {
                service,
                reason: SpawnFailureReason::Other,
                source: std::io::Error::other("HealthCheck: select resolved neither arm"),
            })
        }
    }
}

/// Render the per-engine config text, selected by `service`.
fn render_service_config(
    service: Service,
    port: u16,
    datadir: &std::path::Path,
    socket: &std::path::Path,
    log_path: &std::path::Path,
    init_file: &std::path::Path,
) -> String {
    match service {
        Service::Redis => config_render::render_redis_conf(port, datadir, log_path),
        Service::MySql | Service::MariaDb => {
            config_render::render_my_cnf(port, datadir, socket, log_path, init_file)
        }
        Service::Postgres => config_render::render_postgresql_conf(port, datadir),
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

/// Build the server command per engine, forcing foreground operation and (on
/// Unix) its own process group so the supervisor's `killpg` reaps any children
/// with it. Fallible because the Postgres arm opens the log file for stderr
/// capture.
fn build_cmd(
    service: Service,
    binary: &std::path::Path,
    config_path: &std::path::Path,
    datadir: &std::path::Path,
    log_path: &std::path::Path,
) -> Result<StdCommand, ServiceError> {
    let mut cmd = StdCommand::new(binary);
    match service {
        Service::Redis => {
            cmd.arg(config_path);
        }
        Service::MySql | Service::MariaDb => {
            cmd.arg(format!("--defaults-file={}", config_path.display()));
        }
        Service::Postgres => {
            cmd.arg("-D")
                .arg(datadir)
                .arg("-c")
                .arg(format!("config_file={}", config_path.display()));
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
                .map_err(|source| ServiceError::ConfigWrite {
                    path: log_path.to_path_buf(),
                    service,
                    source,
                })?;
            let f2 = f.try_clone().map_err(|source| ServiceError::ConfigWrite {
                path: log_path.to_path_buf(),
                service,
                source,
            })?;
            cmd.stdout(std::process::Stdio::from(f2));
            cmd.stderr(std::process::Stdio::from(f));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd)
}

/// Arguments for an engine's one-shot datadir init tool. `basedir` is the
/// engine's install root and `staging` is the fresh datadir being populated.
///
/// `mariadb-install-db` is a shell script that resolves its helper binaries
/// (`bin/my_print_defaults`, ...) relative to `--basedir`, defaulting to the
/// current working directory when the flag is absent. Since the daemon's cwd is
/// arbitrary, the flag is mandatory - without it the tool dies with "FATAL
/// ERROR: Could not find `./bin/my_print_defaults`". `mysqld` and `initdb` are
/// self-locating C binaries and need no basedir. Returns an empty vec for an
/// engine with no init step (`Redis`).
fn init_args(
    service: Service,
    basedir: &std::path::Path,
    staging: &std::path::Path,
) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;
    match service {
        Service::Redis => Vec::new(),
        Service::MySql => vec![
            OsString::from("--initialize-insecure"),
            OsString::from(format!("--datadir={}", staging.display())),
        ],
        Service::MariaDb => vec![
            OsString::from(format!("--basedir={}", basedir.display())),
            OsString::from(format!("--datadir={}", staging.display())),
            OsString::from("--auth-root-authentication-method=normal"),
        ],
        Service::Postgres => vec![
            OsString::from("-D"),
            staging.as_os_str().to_os_string(),
            OsString::from("--auth=trust"),
            OsString::from("-U"),
            OsString::from("postgres"),
            OsString::from("-E"),
            OsString::from("UTF8"),
        ],
    }
}

/// Whether `datadir` already holds an initialised instance of `service`.
fn is_initialized(datadir: &std::path::Path, service: Service) -> bool {
    match service {
        Service::Redis => true,
        Service::Postgres => datadir.join("PG_VERSION").is_file(),
        Service::MySql | Service::MariaDb => datadir.join("mysql").is_dir(),
    }
}

/// Refuse to start Postgres against a datadir initialised by a different major
/// version (on-disk format is major-incompatible; cross-major migration is out
/// of scope). A missing/unreadable `PG_VERSION` is treated as "no opinion".
fn check_pg_major(datadir: &std::path::Path, version: &ServiceVersion) -> Result<(), ServiceError> {
    if let Ok(content) = std::fs::read_to_string(datadir.join("PG_VERSION")) {
        let on_disk = content.trim();
        let want = version.major();
        if !on_disk.is_empty() && on_disk != want {
            return Err(ServiceError::Init {
                service: Service::Postgres,
                datadir: datadir.to_path_buf(),
                detail: format!(
                    "datadir was initialised by PostgreSQL {on_disk}, but {want} was requested; \
                     cross-major migration is unsupported"
                ),
            });
        }
    }
    Ok(())
}

/// How `service` is gracefully stopped. Postgres needs SIGINT to the postmaster
/// ("fast shutdown"); SIGTERM to the postmaster is "smart shutdown" and hangs
/// while a client is connected. Every other engine stops cleanly on a group
/// SIGTERM.
fn stop_protocol(service: Service) -> StopProtocol {
    match service {
        Service::Postgres => StopProtocol::MasterInterrupt,
        Service::Redis | Service::MySql | Service::MariaDb => StopProtocol::GroupTerm,
    }
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("config"),
            data: tmp.join("data"),
            state: tmp.join("state"),
            cache: tmp.join("cache"),
            runtime: tmp.join("run"),
        }
    }

    fn v(s: &str) -> ServiceVersion {
        ServiceVersion::from_str(s).unwrap()
    }

    #[test]
    fn stop_protocol_selects_master_interrupt_for_postgres_only() {
        assert_eq!(
            stop_protocol(Service::Postgres),
            StopProtocol::MasterInterrupt
        );
        for service in [Service::Redis, Service::MySql, Service::MariaDb] {
            assert_eq!(stop_protocol(service), StopProtocol::GroupTerm);
        }
    }

    /// Redis has no datadir bootstrap, so it is initialised even with no files present.
    #[test]
    fn is_initialized_redis_is_always_true() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(is_initialized(tmp.path(), Service::Redis));
    }

    #[test]
    fn is_initialized_postgres_keys_on_pg_version_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_initialized(tmp.path(), Service::Postgres));
        std::fs::write(tmp.path().join("PG_VERSION"), b"16\n").unwrap();
        assert!(is_initialized(tmp.path(), Service::Postgres));
    }

    #[test]
    fn is_initialized_mysql_family_keys_on_mysql_dir() {
        let tmp = tempfile::tempdir().unwrap();
        for service in [Service::MySql, Service::MariaDb] {
            let datadir = tmp.path().join(service.id());
            std::fs::create_dir_all(&datadir).unwrap();
            assert!(!is_initialized(&datadir, service));
            std::fs::create_dir_all(datadir.join("mysql")).unwrap();
            assert!(is_initialized(&datadir, service));
        }
    }

    #[test]
    fn check_pg_major_accepts_matching_or_missing_marker() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check_pg_major(tmp.path(), &v("16.2")).is_ok());
        std::fs::write(tmp.path().join("PG_VERSION"), b"16\n").unwrap();
        assert!(check_pg_major(tmp.path(), &v("16.4")).is_ok());
    }

    #[test]
    fn check_pg_major_rejects_cross_major_datadir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("PG_VERSION"), b"15\n").unwrap();
        let err = check_pg_major(tmp.path(), &v("16")).unwrap_err();
        assert!(
            matches!(
                err,
                ServiceError::Init {
                    service: Service::Postgres,
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn starting_attempts_reads_the_counter_or_zero() {
        assert_eq!(
            starting_attempts(PoolState::Starting {
                attempts: 3,
                pid: Some(7),
            }),
            3
        );
        assert_eq!(starting_attempts(PoolState::Stopped), 0);
        assert_eq!(starting_attempts(PoolState::Running { pid: 1 }), 0);
    }

    #[test]
    fn failed_reason_reads_last_exit_or_unknown() {
        assert_eq!(
            failed_reason(PoolState::Failed {
                last_exit: ExitReason::Code(7),
                attempts: 2,
            }),
            ExitReason::Code(7)
        );
        assert_eq!(failed_reason(PoolState::Stopped), ExitReason::Unknown);
    }

    #[test]
    fn render_service_config_embeds_the_port_per_engine() {
        let datadir = std::path::Path::new("/d");
        let socket = std::path::Path::new("/s/x.sock");
        let log = std::path::Path::new("/l/x.log");
        let init = std::path::Path::new("/i/x-init.sql");
        for service in Service::ALL {
            let rendered = render_service_config(service, 6543, datadir, socket, log, init);
            assert!(
                rendered.contains("6543"),
                "{service} config should mention the port:\n{rendered}"
            );
        }
    }

    #[test]
    fn build_cmd_redis_passes_only_the_config_path() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("valkey-server");
        let config = tmp.path().join("redis.conf");
        let datadir = tmp.path().join("data");
        let log = tmp.path().join("redis.log");
        let cmd = build_cmd(Service::Redis, &binary, &config, &datadir, &log).unwrap();
        assert_eq!(cmd.get_program(), binary.as_os_str());
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec![config.as_os_str()]);
    }

    #[test]
    fn build_cmd_mysql_passes_defaults_file_first() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("mysqld");
        let config = tmp.path().join("my.cnf");
        let datadir = tmp.path().join("data");
        let log = tmp.path().join("mysql.log");
        let cmd = build_cmd(Service::MySql, &binary, &config, &datadir, &log).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 1);
        assert!(args[0].starts_with("--defaults-file="), "got: {args:?}");
        assert!(args[0].contains("my.cnf"));
    }

    /// The Postgres arm opens the log file for stderr capture, creating it.
    #[test]
    fn build_cmd_postgres_opens_log_and_sets_datadir_args() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("postgres");
        let config = tmp.path().join("postgresql.conf");
        let datadir = tmp.path().join("data");
        let log = tmp.path().join("pg.log");
        let cmd = build_cmd(Service::Postgres, &binary, &config, &datadir, &log).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-D");
        assert_eq!(args[1], datadir.to_string_lossy());
        assert_eq!(args[2], "-c");
        assert!(args[3].starts_with("config_file="));
        assert!(log.is_file());
    }

    #[test]
    fn build_cmd_postgres_errors_when_log_parent_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("postgres");
        let config = tmp.path().join("postgresql.conf");
        let datadir = tmp.path().join("data");
        let log = tmp.path().join("missing").join("pg.log");
        let err = build_cmd(Service::Postgres, &binary, &config, &datadir, &log).unwrap_err();
        assert!(
            matches!(
                err,
                ServiceError::ConfigWrite {
                    service: Service::Postgres,
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    /// Regression for the `mariadb-install-db` "Could not find
    /// `./bin/my_print_defaults`" failure: it must receive `--basedir` so it
    /// can locate its helper binaries regardless of the daemon's cwd.
    #[test]
    fn init_args_mariadb_passes_basedir_pointing_at_the_install_root() {
        let basedir = std::path::Path::new("/x/services/mariadb/11.4");
        let staging = std::path::Path::new("/x/services/mariadb/.init-staging-1");
        let args: Vec<_> = init_args(Service::MariaDb, basedir, staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "--basedir=/x/services/mariadb/11.4");
        assert!(args.contains(&"--datadir=/x/services/mariadb/.init-staging-1".to_string()));
        assert!(args.contains(&"--auth-root-authentication-method=normal".to_string()));
    }

    /// `mysqld --initialize-insecure` is self-locating and takes no basedir.
    #[test]
    fn init_args_mysql_initializes_insecurely_without_basedir() {
        let basedir = std::path::Path::new("/x/services/mysql/8.4");
        let staging = std::path::Path::new("/x/staging");
        let args: Vec<_> = init_args(Service::MySql, basedir, staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["--initialize-insecure", "--datadir=/x/staging"]);
    }

    #[test]
    fn init_args_postgres_targets_the_staging_dir() {
        let basedir = std::path::Path::new("/x/services/postgres/16");
        let staging = std::path::Path::new("/x/staging");
        let args: Vec<_> = init_args(Service::Postgres, basedir, staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-D");
        assert_eq!(args[1], "/x/staging");
    }

    /// Redis has no init step, so it yields no args (and `run_init` no-ops).
    #[test]
    fn init_args_redis_is_empty() {
        let p = std::path::Path::new("/x");
        assert!(init_args(Service::Redis, p, p).is_empty());
    }

    /// The version path helpers compose the `install_dir/bin` layout that `init_datadir` relies on.
    #[test]
    fn init_datadir_paths_resolve_under_service_root() {
        let dirs = dirs_in(std::path::Path::new("/x"));
        let bin = version::install_dir(&dirs, Service::Postgres, &v("16"))
            .join("bin")
            .join("initdb");
        assert!(bin.ends_with("services/postgres/16/bin/initdb"));
    }
}
