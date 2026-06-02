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
//! - A **one-time datadir init** seam before first start: no-op for Redis;
//!   `initdb` (Postgres) / `mysqld --initialize-insecure` (`MySQL`) /
//!   `mariadb-install-db` (`MariaDB`), run crash-safely into a staging dir.
//!
//! Supervises **Redis (Valkey)**, **`MySQL`**, **`MariaDB`**, and
//! **`PostgreSQL`** — per-engine config rendering, datadir init, and protocol
//! readiness probes are selected from the [`Service`]. (`MariaDB` is not yet
//! published in the services listing, so it installs only once a build exists,
//! but its supervision path is identical to `MySQL`.)

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

        // Fast path: already Running and the child is still alive. Checked
        // before any datadir work so a running engine is never re-initialised.
        if let Some(listen) = self.running_listen(service)? {
            return Ok(listen);
        }

        // Resolve the on-disk layout up front; init and config rendering need it.
        let datadir = version::datadir(&self.dirs, service, &version);
        let config_path = version::config_path(&self.dirs, service);
        let log_path = version::log_path(&self.dirs, service);
        let socket = version::socket_path(&self.dirs, service);

        // One-time datadir initialisation for the SQL engines (no-op otherwise).
        self.init_datadir_if_needed(service, &version, &datadir, &log_path)
            .await?;

        // Fixed loopback port; pre-flight for conflicts.
        self.preflight_port(service, port)?;
        let listen = Listen::TcpLoopback(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

        // Datadir + config/log (+ MySQL/MariaDB socket) parent directories.
        Self::prepare_dirs(service, &datadir, &config_path, &log_path, &socket)?;

        // Render + write the per-engine config.
        let rendered = render_service_config(service, port, &datadir, &socket, &log_path);
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
            // Defence in depth: never point a new major at an incompatible
            // on-disk datadir (the per-major path already avoids this for PG).
            if service == Service::Postgres {
                check_pg_major(datadir, version)?;
            }
            return Ok(());
        }
        // The init log redirect + staging sibling need the log parent.
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
    /// socket) parent directories. The datadir create is idempotent — SQL `init`
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
        // The MySQL/MariaDB socket lives under the (short) runtime dir; its
        // parent must exist before the server creates the socket there.
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
    /// it onto the final datadir — so an interrupted init never leaves a
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
        let init_bin = version::install_dir(&self.dirs, service, version)
            .join("bin")
            .join(init_bin_name);
        if !init_bin.is_file() {
            return Err(ServiceError::Init {
                service,
                datadir: datadir.to_path_buf(),
                detail: format!("install is missing bin/{init_bin_name}"),
            });
        }

        // Fresh, empty staging sibling of the datadir (same filesystem → atomic
        // rename). Mirrors the install staging+swap in `service_install.rs`.
        let staging = version::service_root(&self.dirs, service)
            .join(format!(".init-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| ServiceError::Init {
            service,
            datadir: datadir.to_path_buf(),
            detail: format!("create staging dir: {e}"),
        })?;

        if let Err(e) = self
            .run_init(service, &init_bin, &staging, datadir, log_path)
            .await
        {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        // Swap into place: drop any prior datadir, then rename staging on top.
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
        staging: &std::path::Path,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        let mut cmd = StdCommand::new(init_bin);
        match service {
            Service::MySql => {
                cmd.arg("--initialize-insecure")
                    .arg(format!("--datadir={}", staging.display()));
            }
            Service::MariaDb => {
                cmd.arg(format!("--datadir={}", staging.display()))
                    .arg("--auth-root-authentication-method=normal");
            }
            Service::Postgres => {
                cmd.arg("-D")
                    .arg(staging)
                    .arg("--auth=trust")
                    .arg("-U")
                    .arg("postgres")
                    .arg("-E")
                    .arg("UTF8");
            }
            // Engines with no init binary never reach run_init.
            Service::Redis => return Ok(()),
        }
        // Capture init output to the log (best-effort — diagnostics only).
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
            // Unreachable: `tokio::select!` resolves exactly one branch.
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
) -> String {
    match service {
        Service::Redis => config_render::render_redis_conf(port, datadir, log_path),
        Service::MySql | Service::MariaDb => {
            config_render::render_my_cnf(port, datadir, socket, log_path)
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
        // valkey-server <config>; it writes its own logfile via the directive.
        Service::Redis => {
            cmd.arg(config_path);
        }
        // mysqld|mariadbd --defaults-file=<config> (must be the first arg); the
        // cnf carries datadir / port / socket / log-error.
        Service::MySql | Service::MariaDb => {
            cmd.arg(format!("--defaults-file={}", config_path.display()));
        }
        // postgres -D <datadir> -c config_file=<config>; PG logs to stderr
        // (logging_collector=off), which we redirect to the log file.
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
