//! `ServiceManager` - drives the shared supervisor state machine for one
//! supervised instance per **wire id**, doing the real I/O.
//!
//! Instances are keyed by their wire id string (`"redis"` for a single-instance
//! engine, `"reverb:blog"` for a per-site app server). Each instance carries its
//! [`ServiceDefinition`], so all per-type behaviour (command, config, datadir
//! init, readiness protocol, stop protocol, supervisor policy) is dispatched
//! through the trait rather than a closed enum.
//!
//! Mirrors `yerd_php::PhpManager` in shape (it drives the same `yerd_supervise`
//! state machine) but differs where databases differ: a fixed TCP loopback port
//! pre-flighted for conflicts, a per-type [`SupervisorPolicy`], and a one-time
//! datadir init seam before first start (no-op for engines that need none).

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::{Duration, Instant};

use yerd_platform::{ActivePortBinder, PlatformDirs, PlatformError, PortBinder};
use yerd_supervise::supervisor::{
    transition, Action, Elapsed, ErrorTag, Event, KillSignal, PoolState, StopProtocol,
    SupervisorPolicy,
};
use yerd_supervise::{ChildHandle, Clock, ExitReason, Listen, ProcessSpawner, SpawnFailureReason};

use crate::error::ServiceError;
use crate::health::ReadinessProbe;
use crate::service::{LaunchContext, ReadinessKind, ServiceDefinition};
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
    /// The instance wire id (`"redis"`, `"reverb:blog"`).
    pub service: String,
    /// The running version, if the type is versioned.
    pub version: Option<ServiceVersion>,
    /// Whether the server is alive or has died.
    pub state: ServiceRunState,
    /// The server PID, when running.
    pub pid: Option<u32>,
    /// The address the server is configured to listen on.
    pub listen: Option<Listen>,
}

/// One supervised service instance.
struct Instance<Ch: ChildHandle> {
    def: Arc<dyn ServiceDefinition>,
    state: PoolState,
    state_since: Instant,
    version: Option<ServiceVersion>,
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

/// Supervises local services, one instance per wire id.
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
    instances: BTreeMap<String, Instance<S::Child>>,
}

impl<S, C, P> ServiceManager<S, C, P>
where
    S: ProcessSpawner,
    C: Clock,
    P: ReadinessProbe,
{
    /// Construct a new manager. Each instance's [`SupervisorPolicy`] is sourced
    /// from its [`ServiceDefinition`].
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
            instances: BTreeMap::new(),
        }
    }

    /// Ensure the `def` instance identified by `wire_id` is running on `port`,
    /// returning its listen address. Idempotent: if already running and alive,
    /// returns the cached address.
    ///
    /// For a versioned engine, `version` selects the install (its server binary
    /// is resolved and its datadir initialised on first start) and
    /// `program_override`/`cwd` are `None`. For a per-site app server,
    /// `program_override` is the site's PHP CLI binary, `cwd` its document root,
    /// and `version` is `None`.
    #[allow(clippy::too_many_lines)]
    pub async fn ensure(
        &mut self,
        def: Arc<dyn ServiceDefinition>,
        wire_id: &str,
        version: Option<ServiceVersion>,
        port: u16,
        program_override: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) -> Result<Listen, ServiceError> {
        if let Some(listen) = self.running_listen(wire_id)? {
            return Ok(listen);
        }

        let id = def.id();
        let log_path = version::instance_log_path(&self.dirs, wire_id);
        let config_path = version::config_path(&self.dirs, id);
        let socket = version::socket_path(&self.dirs, id);
        let init_file = version::init_file_path(&self.dirs, id);

        let (program, versioned) = if let Some(program) = program_override {
            (program, None)
        } else {
            let v = version.as_ref().ok_or_else(|| ServiceError::Unsupported {
                service: wire_id.to_owned(),
                detail: "a versioned service requires a version".to_owned(),
            })?;
            let bin = def
                .server_binary()
                .ok_or_else(|| ServiceError::Unsupported {
                    service: wire_id.to_owned(),
                    detail: "type has no server binary".to_owned(),
                })?;
            let program = version::server_path(&self.dirs, id, bin, v);
            if !program.is_file() {
                return Err(ServiceError::VersionNotInstalled {
                    service: wire_id.to_owned(),
                    version: v.clone(),
                });
            }
            let datadir = version::datadir(&self.dirs, id, def.datadir_scope(), v);
            (program, Some((v.clone(), datadir)))
        };

        if let Some((v, datadir)) = versioned.as_ref() {
            self.init_datadir_if_needed(def.as_ref(), wire_id, v, datadir, &log_path)
                .await?;
        }

        self.preflight_port(wire_id, port)?;
        let listen = Listen::TcpLoopback(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

        if let Some((v, datadir)) = versioned.as_ref() {
            Self::prepare_dirs(
                def.as_ref(),
                wire_id,
                datadir,
                &config_path,
                &log_path,
                &socket,
            )?;
            if let Some(sql) = def.bootstrap_sql() {
                std::fs::write(&init_file, sql.as_bytes()).map_err(|source| {
                    ServiceError::ConfigWrite {
                        path: init_file.clone(),
                        service: wire_id.to_owned(),
                        source,
                    }
                })?;
            }
            let preload = if def.preloads_bundled_extensions() {
                let install_dir = version::install_dir(&self.dirs, id, v);
                postgres_preload_libraries(&install_dir)
            } else {
                Vec::new()
            };
            if let Some(rendered) =
                def.render_config(port, datadir, &socket, &log_path, &init_file, &preload)
            {
                std::fs::write(&config_path, rendered.as_bytes()).map_err(|source| {
                    ServiceError::ConfigWrite {
                        path: config_path.clone(),
                        service: wire_id.to_owned(),
                        source,
                    }
                })?;
            }
        } else if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                path: parent.to_path_buf(),
                service: wire_id.to_owned(),
                source,
            })?;
        }

        let geo_env = match versioned.as_ref() {
            Some((v, _)) if def.injects_geo_data() => {
                let install_dir = version::install_dir(&self.dirs, id, v);
                geo_data_env(&install_dir, v)
            }
            _ => Vec::new(),
        };

        let datadir_path = versioned
            .as_ref()
            .map(|(_, d)| d.clone())
            .unwrap_or_default();
        let cmd_builder = || -> Result<StdCommand, ServiceError> {
            let ctx = LaunchContext {
                port,
                program: &program,
                config_path: &config_path,
                datadir: &datadir_path,
                log_path: &log_path,
                geo_env: &geo_env,
                cwd: cwd.as_deref(),
            };
            let mut plan = def.plan_launch(&ctx)?;
            set_own_process_group(&mut plan.command);
            if plan.capture_output_to_log {
                attach_log(&mut plan.command, &log_path, wire_id)?;
            }
            Ok(plan.command)
        };

        let readiness = def.readiness();
        let policy = def.supervisor_policy();
        let stop_protocol = def.stop_protocol();
        let initial_since = self.clock.now();
        let result = self
            .drive(
                wire_id,
                PoolState::Stopped,
                initial_since,
                None,
                Event::EnsureRequested,
                &listen,
                readiness,
                Some(&cmd_builder),
                &policy,
                stop_protocol,
            )
            .await?;

        match result.outcome {
            Outcome::Running { child, pid } => {
                self.instances.insert(
                    wire_id.to_owned(),
                    Instance {
                        def: Arc::clone(&def),
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
                service: wire_id.to_owned(),
                reason: SpawnFailureReason::Other,
                source: std::io::Error::other("ensure: drive returned Stopped"),
            }),
        }
    }

    /// Fast path for [`Self::ensure`]: if `wire_id` is recorded `Running` and its
    /// child is still alive, return its listen address; otherwise `None`.
    fn running_listen(&mut self, wire_id: &str) -> Result<Option<Listen>, ServiceError> {
        let Some(inst) = self.instances.get_mut(wire_id) else {
            return Ok(None);
        };
        if !matches!(inst.state, PoolState::Running { .. }) {
            return Ok(None);
        }
        let alive = match inst.child.as_mut() {
            Some(ch) => ch
                .try_wait()
                .map_err(|source| ServiceError::Spawn {
                    service: wire_id.to_owned(),
                    reason: SpawnFailureReason::WaitFailed,
                    source,
                })?
                .is_none(),
            None => false,
        };
        Ok(alive.then(|| inst.listen.clone()))
    }

    /// One-time datadir initialisation for the engines that need it (no-op for
    /// engines that don't and for an already-initialised datadir). MUST run
    /// before the `create_dir_all(datadir)` in [`Self::prepare_dirs`]: the init
    /// tools populate the datadir themselves (via a crash-safe staging + rename)
    /// and refuse a pre-existing one.
    async fn init_datadir_if_needed(
        &mut self,
        def: &dyn ServiceDefinition,
        wire_id: &str,
        version: &ServiceVersion,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        if !def.needs_init() {
            return Ok(());
        }
        if def.is_initialized(datadir) {
            if matches!(def.datadir_scope(), crate::service::DatadirScope::Major) {
                check_pg_major(datadir, version)?;
            }
            return Ok(());
        }
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                path: parent.to_path_buf(),
                service: wire_id.to_owned(),
                source,
            })?;
        }
        self.init_datadir(def, wire_id, version, datadir, log_path)
            .await
    }

    /// Create the datadir plus the config/log (and, for a socket engine, the Unix
    /// socket) parent directories. The datadir create is idempotent - a SQL
    /// `init` already populated it; this is the real creator for Redis (no init).
    fn prepare_dirs(
        def: &dyn ServiceDefinition,
        wire_id: &str,
        datadir: &std::path::Path,
        config_path: &std::path::Path,
        log_path: &std::path::Path,
        socket: &std::path::Path,
    ) -> Result<(), ServiceError> {
        std::fs::create_dir_all(datadir).map_err(|source| ServiceError::Init {
            service: wire_id.to_owned(),
            datadir: datadir.to_path_buf(),
            detail: source.to_string(),
        })?;
        let mut parents = vec![config_path, log_path];
        if def.uses_unix_socket() {
            parents.push(socket);
        }
        for path in parents {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| ServiceError::ConfigWrite {
                    path: parent.to_path_buf(),
                    service: wire_id.to_owned(),
                    source,
                })?;
            }
        }
        Ok(())
    }

    /// Restart the instance: stop it cleanly, then ensure again.
    pub async fn restart(
        &mut self,
        def: Arc<dyn ServiceDefinition>,
        wire_id: &str,
        version: Option<ServiceVersion>,
        port: u16,
        program_override: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) -> Result<Listen, ServiceError> {
        let _ = self.stop(wire_id).await;
        self.ensure(def, wire_id, version, port, program_override, cwd)
            .await
    }

    /// Stop the instance `wire_id`. No-op if there is none.
    pub async fn stop(&mut self, wire_id: &str) -> Result<(), ServiceError> {
        let Some(mut inst) = self.instances.remove(wire_id) else {
            return Ok(());
        };
        let def = Arc::clone(&inst.def);
        let child = inst.child.take();
        let listen = inst.listen.clone();
        let policy = def.supervisor_policy();
        let stop_protocol = def.stop_protocol();
        self.drive(
            wire_id,
            inst.state,
            inst.state_since,
            child,
            Event::StopRequested,
            &listen,
            def.readiness(),
            None,
            &policy,
            stop_protocol,
        )
        .await
        .map(|_| ())
    }

    /// Stop every supervised instance in deterministic order.
    pub async fn shutdown(&mut self) -> Result<(), ServiceError> {
        let ids: Vec<String> = self.instances.keys().cloned().collect();
        let mut first_err: Option<ServiceError> = None;
        for id in ids {
            if let Err(e) = self.stop(&id).await {
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
        for (wire_id, inst) in &mut self.instances {
            let listen = Some(inst.listen.clone());
            let (state, pid) = match (&inst.state, inst.child.as_mut()) {
                (PoolState::Running { pid }, Some(child)) => match child.try_wait() {
                    Ok(None) => (ServiceRunState::Running, Some(*pid)),
                    _ => (ServiceRunState::Failed, None),
                },
                _ => (ServiceRunState::Failed, None),
            };
            out.push(ServiceSnapshot {
                service: wire_id.clone(),
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
    /// [`ServiceError::Bind`].
    fn preflight_port(&self, wire_id: &str, port: u16) -> Result<(), ServiceError> {
        match self.binder.bind(port) {
            Ok(bound) => {
                drop(bound);
                Ok(())
            }
            Err(PlatformError::Bind { source, .. })
                if source.kind() == std::io::ErrorKind::AddrInUse =>
            {
                Err(ServiceError::PortInUse {
                    service: wire_id.to_owned(),
                    port,
                })
            }
            Err(source) => Err(ServiceError::Bind {
                service: wire_id.to_owned(),
                port,
                source,
            }),
        }
    }

    /// One-time datadir initialisation for an engine that needs it. Runs the
    /// engine's init tool into a fresh **staging** dir, then atomically renames
    /// it onto the final datadir - so an interrupted init never leaves a
    /// half-populated datadir behind (only an orphan `.init-staging-*` the next
    /// attempt removes).
    async fn init_datadir(
        &self,
        def: &dyn ServiceDefinition,
        wire_id: &str,
        version: &ServiceVersion,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        let Some(init_bin_name) = def.init_binary() else {
            return Ok(());
        };
        let install_dir = version::install_dir(&self.dirs, def.id(), version);
        let init_bin = install_dir.join("bin").join(init_bin_name);
        if !init_bin.is_file() {
            return Err(ServiceError::Init {
                service: wire_id.to_owned(),
                datadir: datadir.to_path_buf(),
                detail: format!("install is missing bin/{init_bin_name}"),
            });
        }

        let staging = version::service_root(&self.dirs, def.id())
            .join(format!(".init-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| ServiceError::Init {
            service: wire_id.to_owned(),
            datadir: datadir.to_path_buf(),
            detail: format!("create staging dir: {e}"),
        })?;

        if let Err(e) = self
            .run_init(
                def,
                wire_id,
                &init_bin,
                &install_dir,
                &staging,
                datadir,
                log_path,
            )
            .await
        {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        if datadir.exists() {
            if let Err(e) = std::fs::remove_dir_all(datadir) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(ServiceError::Init {
                    service: wire_id.to_owned(),
                    datadir: datadir.to_path_buf(),
                    detail: format!("remove prior datadir: {e}"),
                });
            }
        }
        if let Some(parent) = datadir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ServiceError::Init {
                service: wire_id.to_owned(),
                datadir: datadir.to_path_buf(),
                detail: format!("create datadir parent: {e}"),
            })?;
        }
        std::fs::rename(&staging, datadir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            ServiceError::Init {
                service: wire_id.to_owned(),
                datadir: datadir.to_path_buf(),
                detail: format!("install datadir: {e}"),
            }
        })
    }

    /// Spawn the engine's init tool one-shot (into `staging`), wait for it, and
    /// require a clean `exit 0`. Init output goes to the service log so a failure
    /// is diagnosable. The tool runs with its cwd set to `basedir` (the install
    /// root) so `mariadb-install-db`'s `--basedir=.` resolves its helper binaries
    /// there.
    ///
    /// Guarded by an [`InitGroupReaper`]: if the owning task is dropped mid-init
    /// (daemon shutdown), the init tool's whole process group is killed so a
    /// grandchild it forked can't leak.
    #[allow(clippy::too_many_arguments)]
    async fn run_init(
        &self,
        def: &dyn ServiceDefinition,
        wire_id: &str,
        init_bin: &std::path::Path,
        basedir: &std::path::Path,
        staging: &std::path::Path,
        datadir: &std::path::Path,
        log_path: &std::path::Path,
    ) -> Result<(), ServiceError> {
        let args = def.init_args(staging);
        if args.is_empty() {
            return Ok(());
        }
        let mut cmd = StdCommand::new(init_bin);
        cmd.args(&args);
        cmd.current_dir(basedir);
        set_own_process_group(&mut cmd);
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
                service: wire_id.to_owned(),
                datadir: datadir.to_path_buf(),
                detail: format!("spawn {}: {source}", init_bin.display()),
            })?;
        let mut reaper = InitGroupReaper::arm(child.id());
        let waited = child.wait().await;
        if waited.is_ok() {
            reaper.disarm();
        }
        let reason = waited.map_err(|source| ServiceError::Init {
            service: wire_id.to_owned(),
            datadir: datadir.to_path_buf(),
            detail: format!("wait for init: {source}"),
        })?;
        match reason {
            ExitReason::Code(0) => Ok(()),
            other => Err(ServiceError::Init {
                service: wire_id.to_owned(),
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
        wire_id: &str,
        mut state: PoolState,
        mut state_since: Instant,
        mut child: Option<S::Child>,
        initial: Event,
        listen: &Listen,
        readiness: ReadinessKind,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, ServiceError> + Sync)>,
        policy: &SupervisorPolicy,
        stop_protocol: StopProtocol,
    ) -> Result<DriveResult<S::Child>, ServiceError> {
        let mut pending = initial;
        loop {
            let (next, action) = transition(state, pending, policy);
            if next != state {
                state = next;
                state_since = self.clock.now();
            }

            match action {
                Action::None => {
                    return Self::finish_terminal(state, &mut child, wire_id, state_since);
                }

                Action::Spawn => {
                    pending = self.spawn_child(wire_id, cmd_builder, &mut child)?;
                }

                Action::HealthCheck => {
                    pending = self
                        .health_check(wire_id, listen, readiness, state_since, &mut child)
                        .await?;
                }

                Action::Backoff { wait } => {
                    tokio::time::sleep(wait).await;
                    pending = Event::BackoffElapsed;
                }

                Action::Kill { signal } => {
                    if let Some(ch) = child.as_mut() {
                        ch.kill(signal, stop_protocol).await.map_err(|source| {
                            ServiceError::Kill {
                                service: wire_id.to_owned(),
                                source,
                            }
                        })?;
                    }
                    pending =
                        wait_after_kill(&mut child, state, signal, wire_id, policy.stop_grace)
                            .await?;
                }

                Action::EmitError(ErrorTag::HealthCheckTimedOut) => {
                    return Err(ServiceError::HealthCheckTimedOut {
                        service: wire_id.to_owned(),
                        attempts: starting_attempts(state),
                    });
                }
                Action::EmitError(ErrorTag::PermanentFailure) => {
                    return Err(ServiceError::PermanentFailure {
                        service: wire_id.to_owned(),
                        reason: failed_reason(state),
                    });
                }
            }
        }
    }

    /// Handle `Action::None`: a terminal state yields a [`DriveResult`]; any
    /// other state is a driver-contract violation.
    fn finish_terminal(
        state: PoolState,
        child: &mut Option<S::Child>,
        wire_id: &str,
        state_since: Instant,
    ) -> Result<DriveResult<S::Child>, ServiceError> {
        match state {
            PoolState::Running { pid } => {
                let ch = child.take().ok_or_else(|| ServiceError::Spawn {
                    service: wire_id.to_owned(),
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
                service: wire_id.to_owned(),
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
        wire_id: &str,
        cmd_builder: Option<&(dyn Fn() -> Result<StdCommand, ServiceError> + Sync)>,
        child: &mut Option<S::Child>,
    ) -> Result<Event, ServiceError> {
        let builder = cmd_builder.ok_or_else(|| ServiceError::Spawn {
            service: wire_id.to_owned(),
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
                service: wire_id.to_owned(),
                reason: SpawnFailureReason::from_kind(source.kind()),
                source,
            }),
        }
    }

    /// Handle `Action::HealthCheck`: probe readiness, racing the child's exit,
    /// and return the follow-up event.
    async fn health_check(
        &mut self,
        wire_id: &str,
        listen: &Listen,
        readiness: ReadinessKind,
        state_since: Instant,
        child: &mut Option<S::Child>,
    ) -> Result<Event, ServiceError> {
        let elapsed_now = self.clock.now().saturating_duration_since(state_since);
        if elapsed_now > Duration::from_millis(0) {
            tokio::time::sleep(HEALTH_PROBE_GAP).await;
        }
        let ch = child.as_mut().ok_or_else(|| ServiceError::Spawn {
            service: wire_id.to_owned(),
            reason: SpawnFailureReason::Other,
            source: std::io::Error::other("HealthCheck with no child handle"),
        })?;

        let probe_fut =
            tokio::time::timeout(HEALTH_PROBE_TIMEOUT, self.probe.probe(readiness, listen));
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
                service: wire_id.to_owned(),
                reason: SpawnFailureReason::WaitFailed,
                source,
            })?;
            *child = None;
            Ok(Event::Crashed { reason })
        } else {
            Err(ServiceError::Spawn {
                service: wire_id.to_owned(),
                reason: SpawnFailureReason::Other,
                source: std::io::Error::other("HealthCheck: select resolved neither arm"),
            })
        }
    }
}

/// Open the log file and attach it to the command's stdout+stderr. Used by the
/// manager when a [`crate::service::LaunchPlan`] requests output capture (the
/// engines that log to their stdio, e.g. Postgres and Reverb).
fn attach_log(
    cmd: &mut StdCommand,
    log_path: &std::path::Path,
    wire_id: &str,
) -> Result<(), ServiceError> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|source| ServiceError::ConfigWrite {
            path: log_path.to_path_buf(),
            service: wire_id.to_owned(),
            source,
        })?;
    let f2 = f.try_clone().map_err(|source| ServiceError::ConfigWrite {
        path: log_path.to_path_buf(),
        service: wire_id.to_owned(),
        source,
    })?;
    cmd.stdout(std::process::Stdio::from(f2));
    cmd.stderr(std::process::Stdio::from(f));
    Ok(())
}

/// Post-kill follow-up: wait for the child to exit (with or without a grace
/// budget) and return the synthetic event the supervisor expects next.
async fn wait_after_kill<Ch: ChildHandle>(
    child: &mut Option<Ch>,
    state: PoolState,
    signal: KillSignal,
    wire_id: &str,
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
                        service: wire_id.to_owned(),
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
                    service: wire_id.to_owned(),
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
                    service: wire_id.to_owned(),
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

/// Put `cmd` in its own process group at spawn time (Unix) so a signal it or any
/// descendant raises can never reach the daemon, and so the supervisor's
/// `killpg(pid, ..)` targets exactly this subtree.
fn set_own_process_group(cmd: &mut StdCommand) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Drop guard that sends SIGKILL to the process group led by a spawned init tool
/// unless [`disarm`](Self::disarm)ed.
struct InitGroupReaper(Option<u32>);

impl InitGroupReaper {
    fn arm(leader_pid: u32) -> Self {
        Self(Some(leader_pid))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for InitGroupReaper {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            yerd_supervise::kill_process_group(pid);
        }
    }
}

/// Environment to inject into the postmaster for a `PostGIS`-bearing postgres
/// variant install: `PROJ_DATA` / `GDAL_DATA`, probed from the install tree.
/// Empty for the base build (no variant suffix); each var is set only when its
/// data file is found. The caller only invokes this for a type that injects geo
/// data (Postgres).
fn geo_data_env(
    install_dir: &std::path::Path,
    version: &ServiceVersion,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    if !version.has_variant() {
        return Vec::new();
    }
    let (proj, gdal) = find_geo_data_dirs(install_dir);
    let mut env = Vec::new();
    if let Some(dir) = proj {
        env.push(("PROJ_DATA".into(), dir.into_os_string()));
    }
    if let Some(dir) = gdal {
        env.push(("GDAL_DATA".into(), dir.into_os_string()));
    }
    env
}

/// The extensions a postgres install needs listed in `shared_preload_libraries`,
/// probed from the install tree. `TimescaleDB` FATAL-errors on `CREATE EXTENSION`
/// unless it is preloaded at postmaster start (a reload is too late), so when the
/// install ships it the library must be named here, with `timescaledb` first per
/// upstream guidance. Only the Linux/macOS `full` build bundles it (the Windows
/// `full` build ships `PostGIS` but not `TimescaleDB`), so this probes the tree
/// rather than keying off the `-full` label. Empty when absent, so a base install
/// never gets a preload line (naming a missing library stops the postmaster
/// starting).
fn postgres_preload_libraries(install_dir: &std::path::Path) -> Vec<&'static str> {
    if timescaledb_present(install_dir) {
        vec!["timescaledb"]
    } else {
        Vec::new()
    }
}

/// Whether a postgres install ships `TimescaleDB`: its `timescaledb.control` file
/// under `share/` or a `timescaledb*.so`/`.dylib` under `lib/`. Fast path checks
/// the standard `share/postgresql/extension/` location, then falls back to a
/// symlink-safe depth-first walk (mirroring [`find_geo_data_dirs`]).
fn timescaledb_present(install_dir: &std::path::Path) -> bool {
    let standard = install_dir
        .join("share")
        .join("postgresql")
        .join("extension")
        .join("timescaledb.control");
    if standard.is_file() {
        return true;
    }
    let mut stack = vec![install_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() && is_timescaledb_file(&entry.file_name()) {
                return true;
            }
        }
    }
    false
}

/// Whether a file name marks a bundled `TimescaleDB`: the extension control file,
/// or a versioned shared object (`timescaledb-2.17.so`, `timescaledb.dylib`, …).
fn is_timescaledb_file(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if name == "timescaledb.control" {
        return true;
    }
    name.starts_with("timescaledb")
        && std::path::Path::new(name)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("so") || ext.eq_ignore_ascii_case("dylib"))
}

/// The directories holding `proj.db` (PROJ) and `gdalvrt.xsd` (GDAL) inside a
/// variant install. Fast path probes `share/proj` / `share/gdal` directly, then
/// falls back to a depth-first walk. `DirEntry::file_type()` does not follow
/// symlinks, so symlinked directories are not recursed (no loop risk).
fn find_geo_data_dirs(
    root: &std::path::Path,
) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    let proj_dir = root.join("share").join("proj");
    let gdal_dir = root.join("share").join("gdal");
    if proj_dir.join("proj.db").is_file() && gdal_dir.join("gdalvrt.xsd").is_file() {
        return (Some(proj_dir), Some(gdal_dir));
    }

    let mut proj = None;
    let mut gdal = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                match entry.file_name().to_str() {
                    Some("proj.db") if proj.is_none() => {
                        proj = entry.path().parent().map(std::path::Path::to_path_buf);
                    }
                    Some("gdalvrt.xsd") if gdal.is_none() => {
                        gdal = entry.path().parent().map(std::path::Path::to_path_buf);
                    }
                    _ => {}
                }
            }
            if proj.is_some() && gdal.is_some() {
                return (proj, gdal);
            }
        }
    }
    (proj, gdal)
}

/// Refuse to start Postgres against a datadir initialised by a different major
/// version (on-disk format is major-incompatible). A missing/unreadable
/// `PG_VERSION` is treated as "no opinion".
fn check_pg_major(datadir: &std::path::Path, version: &ServiceVersion) -> Result<(), ServiceError> {
    if let Ok(content) = std::fs::read_to_string(datadir.join("PG_VERSION")) {
        let on_disk = content.trim();
        let want = version.major();
        if !on_disk.is_empty() && on_disk != want {
            return Err(ServiceError::Init {
                service: "postgres".to_owned(),
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
    fn check_pg_major_accepts_matching_or_missing_marker() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check_pg_major(tmp.path(), &v("16.2")).is_ok());
        std::fs::write(tmp.path().join("PG_VERSION"), b"16\n").unwrap();
        assert!(check_pg_major(tmp.path(), &v("16.4")).is_ok());
    }

    #[test]
    fn check_pg_major_accepts_a_variant_of_the_same_major() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("PG_VERSION"), b"17\n").unwrap();
        assert!(check_pg_major(tmp.path(), &v("17-full")).is_ok());
        assert!(check_pg_major(tmp.path(), &v("17.10-full")).is_ok());
    }

    #[test]
    fn check_pg_major_rejects_cross_major_datadir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("PG_VERSION"), b"15\n").unwrap();
        let err = check_pg_major(tmp.path(), &v("16")).unwrap_err();
        assert!(matches!(err, ServiceError::Init { .. }), "got: {err:?}");
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

    /// An isolated child leads its own process group (`pid == pgid`); an
    /// un-isolated one inherits the parent's group and is not the leader.
    #[cfg(unix)]
    #[test]
    fn set_own_process_group_makes_child_its_own_group_leader() {
        fn own_pid_and_group(isolate: bool) -> (i32, i32) {
            let mut cmd = StdCommand::new("sh");
            cmd.arg("-c")
                .arg("printf '%s %s' \"$$\" \"$(ps -o pgid= -p $$)\"");
            if isolate {
                set_own_process_group(&mut cmd);
            }
            let out = cmd.output().expect("spawn sh");
            let text = String::from_utf8_lossy(&out.stdout);
            let mut fields = text.split_whitespace();
            let child_pid = fields.next().unwrap().parse().unwrap();
            let group = fields.next().unwrap().parse().unwrap();
            (child_pid, group)
        }

        let (child_pid, group) = own_pid_and_group(true);
        assert_eq!(child_pid, group, "isolated child must lead its own group");

        let (child_pid, group) = own_pid_and_group(false);
        assert_ne!(child_pid, group, "un-isolated child is not the leader");
    }

    #[test]
    fn init_group_reaper_disarm_clears_the_pid() {
        let mut reaper = InitGroupReaper::arm(4242);
        let armed = reaper.0;
        reaper.disarm();
        let disarmed = reaper.0;
        assert_eq!(armed, Some(4242));
        assert_eq!(disarmed, None);
    }

    /// `run_init` must spawn the datadir-init tool into its **own** process group,
    /// or a group-directed signal from a shell-script init tool reaches the daemon.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_init_spawns_the_init_tool_in_its_own_process_group() {
        use std::os::unix::fs::PermissionsExt;
        use yerd_supervise::{SystemClock, TokioProcessSpawner};

        let tmp = tempfile::tempdir().unwrap();
        let init_bin = tmp.path().join("fake-init.sh");
        std::fs::write(
            &init_bin,
            "#!/bin/sh\nprintf '%s %s' \"$$\" \"$(ps -o pgid= -p $$)\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&init_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = tmp.path().join("init.log");
        let mgr = ServiceManager::new(
            TokioProcessSpawner,
            SystemClock,
            crate::health::ServiceProbes::new(),
            dirs_in(tmp.path()),
            ActivePortBinder::new(),
        );
        mgr.run_init(
            &crate::service::MySql,
            "mysql",
            &init_bin,
            tmp.path(),
            &tmp.path().join("staging"),
            &tmp.path().join("datadir"),
            &log,
        )
        .await
        .expect("fake init exits 0");

        let out = std::fs::read_to_string(&log).unwrap();
        let mut fields = out.split_whitespace();
        let child_pid: i32 = fields.next().unwrap().parse().unwrap();
        let group: i32 = fields.next().unwrap().parse().unwrap();
        assert_eq!(
            child_pid, group,
            "run_init must put the init tool in its own group"
        );
    }

    /// `run_init` must run the init tool with its cwd set to the install root
    /// (`basedir`), so `mariadb-install-db`'s `--basedir=.` resolves there.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_init_runs_the_init_tool_from_the_install_root() {
        use std::os::unix::fs::PermissionsExt;
        use yerd_supervise::{SystemClock, TokioProcessSpawner};

        let tmp = tempfile::tempdir().unwrap();
        let basedir = tmp.path().join("install root");
        std::fs::create_dir_all(&basedir).unwrap();
        let init_bin = tmp.path().join("fake-init.sh");
        std::fs::write(&init_bin, "#!/bin/sh\npwd -P\nexit 0\n").unwrap();
        std::fs::set_permissions(&init_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = tmp.path().join("init.log");
        let mgr = ServiceManager::new(
            TokioProcessSpawner,
            SystemClock,
            crate::health::ServiceProbes::new(),
            dirs_in(tmp.path()),
            ActivePortBinder::new(),
        );
        mgr.run_init(
            &crate::service::MySql,
            "mysql",
            &init_bin,
            &basedir,
            &tmp.path().join("staging"),
            &tmp.path().join("datadir"),
            &log,
        )
        .await
        .expect("fake init exits 0");

        let reported = std::fs::read_to_string(&log).unwrap();
        let want = std::fs::canonicalize(&basedir).unwrap();
        assert_eq!(reported.trim(), want.to_string_lossy());
    }

    /// A staged install tree with the two geo-data files under `share/`.
    fn stage_geo_install(root: &std::path::Path, proj: bool, gdal: bool) {
        if proj {
            let dir = root.join("share").join("proj");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("proj.db"), b"").unwrap();
        }
        if gdal {
            let dir = root.join("share").join("gdal");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("gdalvrt.xsd"), b"").unwrap();
        }
    }

    #[test]
    fn geo_data_env_empty_for_base_label() {
        let tmp = tempfile::tempdir().unwrap();
        stage_geo_install(tmp.path(), true, true);
        assert!(geo_data_env(tmp.path(), &v("17")).is_empty());
    }

    #[test]
    fn geo_data_env_sets_both_when_probed() {
        let tmp = tempfile::tempdir().unwrap();
        stage_geo_install(tmp.path(), true, true);
        let env = geo_data_env(tmp.path(), &v("17-full"));
        let map: std::collections::BTreeMap<_, _> = env.into_iter().collect();
        assert_eq!(
            map.get(std::ffi::OsStr::new("PROJ_DATA"))
                .map(std::ffi::OsString::as_os_str),
            Some(tmp.path().join("share").join("proj").as_os_str())
        );
        assert_eq!(
            map.get(std::ffi::OsStr::new("GDAL_DATA"))
                .map(std::ffi::OsString::as_os_str),
            Some(tmp.path().join("share").join("gdal").as_os_str())
        );
    }

    #[test]
    fn geo_data_env_sets_only_the_found_var() {
        let tmp = tempfile::tempdir().unwrap();
        stage_geo_install(tmp.path(), true, false);
        let env = geo_data_env(tmp.path(), &v("17-full"));
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, std::ffi::OsString::from("PROJ_DATA"));
    }

    #[test]
    fn find_geo_data_dirs_falls_back_to_walk_for_nonstandard_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let proj_dir = tmp.path().join("lib").join("proj9");
        let gdal_dir = tmp.path().join("share").join("contrib").join("gdal");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::create_dir_all(&gdal_dir).unwrap();
        std::fs::write(proj_dir.join("proj.db"), b"").unwrap();
        std::fs::write(gdal_dir.join("gdalvrt.xsd"), b"").unwrap();
        assert_eq!(
            find_geo_data_dirs(tmp.path()),
            (Some(proj_dir), Some(gdal_dir))
        );
    }

    /// Stage the standard extension dir and drop `timescaledb.control` into it.
    fn stage_timescaledb_control(root: &std::path::Path) {
        let dir = root.join("share").join("postgresql").join("extension");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("timescaledb.control"), b"").unwrap();
    }

    #[test]
    fn preload_libraries_empty_without_timescaledb() {
        let tmp = tempfile::tempdir().unwrap();
        stage_geo_install(tmp.path(), true, true);
        assert!(postgres_preload_libraries(tmp.path()).is_empty());
    }

    #[test]
    fn preload_libraries_lists_timescaledb_when_control_present() {
        let tmp = tempfile::tempdir().unwrap();
        stage_timescaledb_control(tmp.path());
        assert_eq!(postgres_preload_libraries(tmp.path()), vec!["timescaledb"]);
    }

    #[test]
    fn timescaledb_present_detects_a_versioned_shared_object() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib").join("postgresql");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("timescaledb-2.17.so"), b"").unwrap();
        assert!(timescaledb_present(tmp.path()));
    }

    #[test]
    fn timescaledb_present_detects_a_dylib() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("timescaledb.dylib"), b"").unwrap();
        assert!(timescaledb_present(tmp.path()));
    }

    #[test]
    fn timescaledb_present_false_for_bare_install() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("postgis.so"), b"").unwrap();
        assert!(!timescaledb_present(tmp.path()));
    }

    #[test]
    fn init_datadir_paths_resolve_under_service_root() {
        let dirs = dirs_in(std::path::Path::new("/x"));
        let bin = version::install_dir(&dirs, "postgres", &v("16"))
            .join("bin")
            .join("initdb");
        assert!(bin.ends_with("services/postgres/16/bin/initdb"));
    }
}
